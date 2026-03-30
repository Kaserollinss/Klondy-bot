use std::thread;

use rand::rngs::SmallRng;
use rand::SeedableRng;

use lonelybot::convert::convert_moves;
use lonelybot::mcts_solver::pick_moves;
use lonelybot::standard::StandardSolitaire;
use lonelybot::tracking::DefaultTerminateSignal;

#[allow(clippy::cast_precision_loss)]
fn ucb1(n_sucess: usize, n_visit: usize, n_total: usize) -> f64 {
    const C: f64 = 2.;

    if n_visit == 0 {
        f64::INFINITY
    } else if n_sucess == !0 {
        f64::INFINITY
    } else {
        let exploitation = n_sucess as f64 / (n_visit as f64 * 52.0);
        exploitation + C * ((n_total as f64).ln() / n_visit as f64).sqrt()
    }
}

use crate::adapter::{AdapterError, ScreenAdapter};

/// How the driver reports recommended moves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverMode {
    /// Print recommended moves to stdout; user executes manually.
    Advisor,
    /// Automatically execute moves via the adapter.
    AutoPlay,
}

pub struct GameDriver<A: ScreenAdapter> {
    adapter: A,
    mode: DriverMode,
    rng: SmallRng,
    mcts_iterations: usize,
    mcts_limit: usize,
}

impl<A: ScreenAdapter> GameDriver<A> {
    pub fn new(adapter: A, mode: DriverMode) -> Self {
        Self {
            adapter,
            mode,
            rng: SmallRng::seed_from_u64(0),
            mcts_iterations: 3000,
            mcts_limit: 1000,
        }
    }

    pub fn with_seed(mut self, seed: u64) -> Self {
        self.rng = SmallRng::seed_from_u64(seed);
        self
    }

    pub fn with_mcts_params(mut self, iterations: usize, limit: usize) -> Self {
        self.mcts_iterations = iterations;
        self.mcts_limit = limit;
        self
    }

    /// Run one cycle: read board, compute best move, report/execute it.
    /// Returns the recommended move description, or an error.
    pub fn step(&mut self) -> Result<String, AdapterError> {
        // 1. Read board
        let board = self.adapter.read_board()?;
        board.validate().map_err(|e| {
            AdapterError::RecognitionError(format!("invalid board: {e:?}"))
        })?;

        // 2. Build solitaire state
        let solitaire = board.to_solitaire(&mut self.rng);
        let std_game = StandardSolitaire::from(&solitaire);

        // 3. Find best move via MCTS
        let mut solver_state = solitaire.clone();
        solver_state.hidden_clear();
        let best = pick_moves(
            &mut solver_state,
            &mut self.rng,
            self.mcts_iterations,
            self.mcts_limit,
            &DefaultTerminateSignal {},
            ucb1,
        );

        let Some(moves) = best else {
            return Err(AdapterError::GameOver);
        };

        // 4. Convert to standard moves
        let mut std_copy = std_game.clone();
        let std_moves = convert_moves(&mut std_copy, &moves).map_err(|_| {
            AdapterError::RecognitionError("failed to convert moves".into())
        })?;

        if std_moves.is_empty() {
            return Err(AdapterError::GameOver);
        }

        let first_move = &std_moves[0];
        let move_desc = format!("{first_move}");

        // 5. Execute or advise
        match self.mode {
            DriverMode::Advisor => {
                println!("Recommended: {move_desc}");
            }
            DriverMode::AutoPlay => {
                self.adapter.execute_move(first_move)?;
            }
        }

        // 6. Wait for UI
        thread::sleep(self.adapter.settle_time());

        Ok(move_desc)
    }

    /// Run the game loop until completion or error.
    pub fn run(&mut self) -> Result<(), AdapterError> {
        loop {
            match self.step() {
                Ok(m) => {
                    if self.mode == DriverMode::AutoPlay {
                        println!("Played: {m}");
                    }
                }
                Err(AdapterError::GameOver) => {
                    println!("No more moves available.");
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
    }
}
