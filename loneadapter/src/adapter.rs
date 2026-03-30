use std::time::Duration;

use lonelybot::partial::PartialBoard;
use lonelybot::standard::StandardMove;

#[derive(Debug, thiserror::Error)]
pub enum AdapterError {
    #[error("failed to capture screen: {0}")]
    CaptureError(String),
    #[error("failed to recognize board: {0}")]
    RecognitionError(String),
    #[error("failed to execute move: {0}")]
    ExecutionError(String),
    #[error("game is over")]
    GameOver,
}

/// A screen adapter reads game state and optionally executes moves.
pub trait ScreenAdapter {
    /// Capture and recognize the current board state from the screen.
    fn read_board(&mut self) -> Result<PartialBoard, AdapterError>;

    /// Whether this adapter supports executing moves (auto-play).
    fn can_execute(&self) -> bool {
        false
    }

    /// Execute a move by clicking/dragging on screen.
    fn execute_move(&mut self, _m: &StandardMove) -> Result<(), AdapterError> {
        Err(AdapterError::ExecutionError("not supported".into()))
    }

    /// Human-readable name of this adapter.
    fn name(&self) -> &str;

    /// Time to wait for UI animations after a move.
    fn settle_time(&self) -> Duration {
        Duration::from_millis(500)
    }
}
