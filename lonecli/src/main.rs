mod solvitaire;
#[allow(dead_code)]
mod tui;

use bpci::{Interval, NSuccessesSample, WilsonScore};
use clap::{Args, Parser, Subcommand, ValueEnum};
use loneadapter::adapters::solitaire_cash::SolitaireCashAdapter;
use loneadapter::adapters::solitaire_cash_macos::{
    DebugOptions, MacNativeMouse, MacScreenCapture, PapayaSolitaireCashRecognizer, ScreenRegion,
    ScreenshotVisionBackend,
};
use loneadapter::{DriverMode, GameDriver};
use lonelybot::convert::convert_moves;
use lonelybot::engine::SolitaireEngine;
use lonelybot::mcts_solver::pick_moves;
use lonelybot::pruning::NoPruner;
use lonelybot::shuffler::{self, CardDeck, U256};
use lonelybot::solver::{solve_with_tracking, SearchResult};
use lonelybot::state::Solitaire;
use lonelybot::tracking::{BudgetedTerminateSignal, DefaultTerminateSignal, EmptySearchStats};
use rand::prelude::*;
use solvitaire::Solvitaire;
use std::num::NonZeroU8;
use std::path::PathBuf;
use std::time;
use std::time::Duration;

use lonelybot::standard::{StandardMove, StandardSolitaire};

#[derive(ValueEnum, Clone, Copy)]
enum SeedType {
    /// Doc comment
    Default,
    Solvitaire,
    KlondikeSolver,
    Greenfelt,
    Exact,
    Microsoft,
}

#[derive(Args, Clone)]
struct StringSeed {
    seed_type: SeedType,
    seed: String,
}

struct Seed {
    seed_type: SeedType,
    seed: U256,
}

impl From<&StringSeed> for Seed {
    fn from(value: &StringSeed) -> Self {
        Seed {
            seed_type: value.seed_type,
            seed: U256::from_dec_str(&value.seed).unwrap(),
        }
    }
}

impl std::fmt::Display for Seed {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}-{}",
            match self.seed_type {
                SeedType::Default => "L",
                SeedType::Solvitaire => "S",
                SeedType::KlondikeSolver => "K",
                SeedType::Greenfelt => "G",
                SeedType::Exact => "E",
                SeedType::Microsoft => "M",
            },
            self.seed
        )
    }
}

impl Seed {
    #[must_use]
    pub(crate) const fn seed(&self) -> U256 {
        self.seed
    }

    #[must_use]
    pub(crate) fn increase(&self, step: u32) -> Self {
        Self {
            seed_type: self.seed_type,
            seed: self.seed() + step,
        }
    }
}

#[must_use]
fn shuffle(s: &Seed) -> CardDeck {
    let seed = s.seed;
    match s.seed_type {
        SeedType::Default => shuffler::default_shuffle(seed.as_u64()),
        SeedType::Solvitaire => shuffler::solvitaire_shuffle(seed.as_u32()),
        SeedType::KlondikeSolver => shuffler::ks_shuffle(seed.as_u32()),
        SeedType::Greenfelt => shuffler::greenfelt_shuffle(seed.as_u32()),
        SeedType::Exact => shuffler::exact_shuffle(seed).unwrap(),
        SeedType::Microsoft => shuffler::microsoft_shuffle(seed).unwrap(),
    }
}

fn ucb1(n_sucess: usize, n_visit: usize, n_total: usize) -> f64 {
    const C: f64 = 2.;

    #[allow(clippy::cast_precision_loss)]
    if n_visit == 0 {
        f64::INFINITY
    } else if n_sucess == !0 {
        // SURE_WIN sentinel
        f64::INFINITY
    } else {
        let exploitation = n_sucess as f64 / (n_visit as f64 * 52.0);
        exploitation + C * ((n_total as f64).ln() / n_visit as f64).sqrt()
    }
}

#[allow(dead_code)]
enum SolveOutput {
    Solved(Vec<StandardMove>),
    Unsolvable,
    BestEffort(Vec<StandardMove>, u8),
}

fn solve_game(seed: &Seed, draw_step: NonZeroU8, verbose: bool) -> SolveOutput {
    const N_TIMES: usize = 3000;
    const LIMIT: usize = 1000;
    const EXACT_BUDGET: usize = 500_000;

    let cards = shuffle(seed);
    let std_game = StandardSolitaire::new(&cards, draw_step);

    // Phase 1: Try exact solve with budget
    let mut exact_game = Solitaire::new(&cards, draw_step);
    let budget_signal = BudgetedTerminateSignal::new(EXACT_BUDGET);
    let (result, history) = solve_with_tracking(&mut exact_game, &EmptySearchStats {}, &budget_signal);

    match result {
        SearchResult::Solved => {
            let mut std_game_copy = std_game.clone();
            let std_moves = convert_moves(&mut std_game_copy, &history.unwrap()).unwrap();
            if verbose {
                println!("Solved (exact)");
                for m in &std_moves {
                    print!("{m}, ");
                }
                println!();
            }
            return SolveOutput::Solved(std_moves.to_vec());
        }
        SearchResult::Unsolvable => {
            if verbose {
                println!("Proved unsolvable");
            }
            return SolveOutput::Unsolvable;
        }
        _ => {
            if verbose {
                println!("Exact solve exhausted budget, falling back to MCTS...");
            }
        }
    }

    // Phase 2: MCTS fallback with score-guided search
    let mut game: SolitaireEngine<NoPruner> = Solitaire::new(&cards, draw_step).into();
    let mut rng = SmallRng::seed_from_u64(seed.seed().as_u64());
    let mut accumulated_moves = Vec::new();
    let mut best_line: Vec<lonelybot::moves::Move> = Vec::new();
    let mut best_score: u8 = 0;

    while !game.state().is_win() {
        let mut gg = game.state().clone();
        gg.hidden_clear();
        let best = pick_moves(
            &mut gg,
            &mut rng,
            N_TIMES,
            LIMIT,
            &DefaultTerminateSignal {},
            ucb1,
        );
        let Some(best) = best else {
            break;
        };
        if verbose {
            for m in &best {
                print!("{m}, ");
            }
            println!();
        }
        for m in &best {
            accumulated_moves.push(*m);
            game.do_move(*m);
        }

        let current_score = game.state().get_stack().len();
        if current_score > best_score {
            best_score = current_score;
            best_line = accumulated_moves.clone();
        }
    }

    if game.state().is_win() {
        let mut std_game_copy = std_game.clone();
        let std_moves = convert_moves(&mut std_game_copy, &accumulated_moves).unwrap();
        if verbose {
            println!("Solved (MCTS)");
        }
        SolveOutput::Solved(std_moves.to_vec())
    } else {
        if verbose {
            println!("Best effort: {best_score}/52 foundation cards");
        }
        let mut std_game_copy = std_game.clone();
        match convert_moves(&mut std_game_copy, &best_line) {
            Ok(std_moves) => SolveOutput::BestEffort(std_moves.to_vec(), best_score),
            Err(_) => SolveOutput::BestEffort(Vec::new(), best_score),
        }
    }
}

#[derive(Parser)]
#[command(author, version, about, long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    Print {
        #[command(flatten)]
        seed: StringSeed,
    },

    Hop {
        #[command(flatten)]
        seed: StringSeed,
        draw_step: NonZeroU8,
    },
    HopLoop {
        #[command(flatten)]
        seed: StringSeed,
        draw_step: NonZeroU8,
    },
    Screen {
        #[command(subcommand)]
        game: ScreenCommands,
    },
}

#[derive(Subcommand)]
enum ScreenCommands {
    SolitaireCash(ScreenSolitaireCashArgs),
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum ScreenMode {
    Advisor,
    Autoplay,
}

#[derive(Args, Clone)]
struct ScreenSolitaireCashArgs {
    #[arg(long, default_value_t = 0)]
    x: u32,
    #[arg(long, default_value_t = 0)]
    y: u32,
    #[arg(long, default_value_t = 640)]
    width: u32,
    #[arg(long, default_value_t = 980)]
    height: u32,
    #[arg(long, value_enum, default_value_t = ScreenMode::Advisor)]
    mode: ScreenMode,
    #[arg(long)]
    loop_mode: bool,
    #[arg(long, default_value_t = 3000)]
    mcts_iterations: usize,
    #[arg(long, default_value_t = 1000)]
    mcts_limit: usize,
    #[arg(long, default_value_t = 180)]
    scan_delay_ms: u64,
    #[arg(long, default_value_t = 500)]
    settle_ms: u64,
    #[arg(long, default_value_os_t = default_solitaire_cash_assets())]
    assets: PathBuf,
    #[arg(long)]
    debug: bool,
    #[arg(long)]
    debug_dir: Option<PathBuf>,
}

fn default_solitaire_cash_assets() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .join("Img")
}

fn run_solitaire_cash_screen(args: &ScreenSolitaireCashArgs) {
    let region = ScreenRegion::new(args.x, args.y, args.width, args.height);
    let debug = DebugOptions {
        enabled: args.debug,
        dump_dir: args.debug_dir.clone(),
    };

    if args.debug {
        eprintln!(
            "[solitaire-cash] starting mode={:?} loop={} region=({}, {}) -> {}x{} assets={}",
            args.mode,
            args.loop_mode,
            args.x,
            args.y,
            args.width,
            args.height,
            args.assets.display()
        );
        if let Some(dir) = &args.debug_dir {
            eprintln!("[solitaire-cash] debug screenshots will be copied to {}", dir.display());
        }
    }

    let capture = MacScreenCapture::new(region).with_debug(debug.clone());
    let recognizer = PapayaSolitaireCashRecognizer::from_asset_dir(&args.assets)
        .unwrap()
        .with_debug(debug.clone());
    let backend = ScreenshotVisionBackend::new(capture, recognizer, region)
        .with_debug(debug.clone())
        .with_mouse(MacNativeMouse::new());
    let adapter = SolitaireCashAdapter::new(backend)
        .with_scan_tap_delay(Duration::from_millis(args.scan_delay_ms))
        .with_settle_time(Duration::from_millis(args.settle_ms))
        .with_debug(args.debug);
    let mode = match args.mode {
        ScreenMode::Advisor => DriverMode::Advisor,
        ScreenMode::Autoplay => DriverMode::AutoPlay,
    };
    let mut driver = GameDriver::new(adapter, mode)
        .with_mcts_params(args.mcts_iterations, args.mcts_limit);

    if args.loop_mode || matches!(args.mode, ScreenMode::Autoplay) {
        driver.run().unwrap();
    } else {
        let move_desc = driver.step().unwrap();
        println!("{move_desc}");
    }
}

fn main() {
    let args = Cli::parse().command;

    match &args {
        Commands::Print { seed } => {
            let shuffled_deck = shuffle(&seed.into());
            let g = StandardSolitaire::new(&shuffled_deck, NonZeroU8::MIN);

            println!("{}", Solvitaire(g));
        }
        Commands::Hop { seed, draw_step } => {
            solve_game(&seed.into(), *draw_step, true);
        }
        Commands::HopLoop { seed, draw_step } => {
            let mut cnt_solve: u32 = 0;
            for i in 0.. {
                let s: Seed = seed.into();
                let start = time::Instant::now();

                cnt_solve += u32::from(matches!(
                    solve_game(&s.increase(i), *draw_step, false),
                    SolveOutput::Solved(_)
                ));
                let elapsed = start.elapsed();

                let interval = NSuccessesSample::new(i + 1, cnt_solve)
                    .unwrap()
                    .wilson_score(1.960);
                println!(
                    "{}/{} ~ {:.4} < {:.4} < {:.4} in {:?}",
                    cnt_solve,
                    i + 1,
                    interval.lower(),
                    f64::from(cnt_solve) / f64::from(i + 1),
                    interval.upper(),
                    elapsed
                );
            }
        }
        Commands::Screen { game } => match game {
            ScreenCommands::SolitaireCash(args) => run_solitaire_cash_screen(args),
        },
    }
}
