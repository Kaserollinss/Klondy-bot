use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use clap::Args;
use loneadapter::adapters::solitaire_cash::{
    NormalizedRect, Point, SolitaireCashCalibration,
};
use loneadapter::adapters::solitaire_cash_macos::{
    PapayaSolitaireCashRecognizer, SelectedSlotPreview, SlotReport, SlotState,
};
use loneadapter::adapters::solitaire_cash_templates::MatchCandidate;
use loneadapter::AdapterError;
use serde::{Deserialize, Serialize};

const CALIBRATOR_HTML: &str = include_str!("solitaire_cash_calibrator.html");

#[derive(Args, Clone)]
pub struct CalibrateSolitaireCashArgs {
    #[arg(value_name = "PNG")]
    pub image: PathBuf,
    #[arg(long, default_value_os_t = crate::default_solitaire_cash_assets())]
    pub assets: PathBuf,
    #[arg(long, default_value = "127.0.0.1:43123")]
    pub bind: String,
    #[arg(long)]
    pub open: bool,
}

#[derive(Debug, Deserialize)]
struct PreviewRequest {
    calibration: SolitaireCashCalibration,
    selected_slot: Option<String>,
}

#[derive(Debug, Serialize)]
struct PreviewResponse {
    image_url: String,
    image_width: u32,
    image_height: u32,
    calibration: SolitaireCashCalibration,
    export_rust: String,
    board_summary: BoardSummary,
    slots: Vec<SlotSummary>,
    control_points: ControlPoints,
    selected_slot: Option<SelectedSlotSummary>,
}

#[derive(Debug, Serialize)]
struct BoardSummary {
    foundation: [u8; 4],
    waste_visible: Vec<String>,
    stock_present: bool,
    piles: Vec<PileSummary>,
}

#[derive(Debug, Serialize)]
struct PileSummary {
    hidden_count: u8,
    visible_cards: Vec<String>,
}

#[derive(Debug, Serialize)]
struct SlotSummary {
    label: String,
    rect: NormalizedRect,
    state: &'static str,
    card: Option<String>,
    low_confidence: bool,
    rank_candidates: Vec<MatchCandidate>,
    suit_candidates: Vec<MatchCandidate>,
}

#[derive(Debug, Serialize)]
struct ControlPoints {
    stock_tap_point: Point,
    submit_point: Point,
    undo_point: Point,
}

#[derive(Debug, Serialize)]
struct SelectedSlotSummary {
    label: String,
    face_rect: NormalizedRect,
    rank_rect: NormalizedRect,
    suit_rect: NormalizedRect,
    rank_candidates: Vec<MatchCandidate>,
    suit_candidates: Vec<MatchCandidate>,
    rank_raw_url: String,
    suit_raw_url: String,
    rank_mask_url: String,
    suit_mask_url: String,
}

#[derive(Debug)]
struct Artifact {
    content_type: &'static str,
    body: Vec<u8>,
}

struct CalibrationSession {
    image: PathBuf,
    recognizer: Mutex<PapayaSolitaireCashRecognizer>,
    default_calibration: SolitaireCashCalibration,
    artifacts: Mutex<HashMap<String, Artifact>>,
    next_artifact_id: AtomicU64,
}

impl CalibrationSession {
    fn new(
        image: PathBuf,
        recognizer: PapayaSolitaireCashRecognizer,
        default_calibration: SolitaireCashCalibration,
    ) -> Self {
        Self {
            image,
            recognizer: Mutex::new(recognizer),
            default_calibration,
            artifacts: Mutex::new(HashMap::new()),
            next_artifact_id: AtomicU64::new(1),
        }
    }

    fn init_response(&self) -> Result<PreviewResponse, AdapterError> {
        self.preview_response(PreviewRequest {
            calibration: self.default_calibration,
            selected_slot: None,
        })
    }

    fn preview_response(&self, request: PreviewRequest) -> Result<PreviewResponse, AdapterError> {
        let recognizer = self.recognizer.lock().map_err(|_| {
            AdapterError::ExecutionError("failed to lock solitaire cash recognizer".into())
        })?;
        let preview = recognizer.preview_png_with_calibration(
            &self.image,
            &request.calibration,
            request.selected_slot.as_deref(),
        )?;
        drop(recognizer);

        Ok(PreviewResponse {
            image_url: "/screenshot".into(),
            image_width: preview.report.image_width,
            image_height: preview.report.image_height,
            calibration: request.calibration,
            export_rust: request.calibration.to_rust_literal(),
            board_summary: BoardSummary {
                foundation: preview.report.board.foundation,
                waste_visible: preview
                    .report
                    .board
                    .waste
                    .iter()
                    .map(ToString::to_string)
                    .collect(),
                stock_present: preview.report.board.stock_present,
                piles: preview
                    .report
                    .board
                    .piles
                    .iter()
                    .map(|pile| PileSummary {
                        hidden_count: pile.hidden_count,
                        visible_cards: pile.cards.iter().map(ToString::to_string).collect(),
                    })
                    .collect(),
            },
            slots: preview.report.slots.iter().map(Self::slot_summary).collect(),
            control_points: ControlPoints {
                stock_tap_point: request.calibration.layout.stock_tap_point,
                submit_point: request.calibration.layout.submit_point,
                undo_point: request.calibration.layout.undo_point,
            },
            selected_slot: preview
                .selected_slot
                .as_ref()
                .map(|selected| self.selected_slot_summary(selected))
                .transpose()?,
        })
    }

    fn slot_summary(slot: &SlotReport) -> SlotSummary {
        SlotSummary {
            label: slot.label.clone(),
            rect: slot.rect,
            state: match slot.state {
                SlotState::Empty => "Empty",
                SlotState::FaceDown => "FaceDown",
                SlotState::FaceUp => "FaceUp",
                SlotState::Recycle => "Recycle",
            },
            card: slot.card.map(|card| card.to_string()),
            low_confidence: slot.low_confidence,
            rank_candidates: slot.rank_candidates.clone(),
            suit_candidates: slot.suit_candidates.clone(),
        }
    }

    fn selected_slot_summary(
        &self,
        selected: &SelectedSlotPreview,
    ) -> Result<SelectedSlotSummary, AdapterError> {
        Ok(SelectedSlotSummary {
            label: selected.label.clone(),
            face_rect: selected.face_rect,
            rank_rect: selected.rank_rect,
            suit_rect: selected.suit_rect,
            rank_candidates: selected.rank_candidates.clone(),
            suit_candidates: selected.suit_candidates.clone(),
            rank_raw_url: self.store_artifact("rank-raw", selected.rank_raw_png.clone()),
            suit_raw_url: self.store_artifact("suit-raw", selected.suit_raw_png.clone()),
            rank_mask_url: self.store_artifact("rank-mask", selected.rank_mask_png.clone()),
            suit_mask_url: self.store_artifact("suit-mask", selected.suit_mask_png.clone()),
        })
    }

    fn store_artifact(&self, stem: &str, body: Vec<u8>) -> String {
        let id = self.next_artifact_id.fetch_add(1, Ordering::Relaxed);
        let key = format!("{stem}-{id}.png");
        if let Ok(mut artifacts) = self.artifacts.lock() {
            artifacts.insert(
                key.clone(),
                Artifact {
                    content_type: "image/png",
                    body,
                },
            );
        }
        format!("/artifact/{key}")
    }

    fn screenshot_bytes(&self) -> Result<Vec<u8>, AdapterError> {
        fs::read(&self.image).map_err(|err| {
            AdapterError::CaptureError(format!(
                "failed to read screenshot {}: {err}",
                self.image.display()
            ))
        })
    }
}

pub fn run_solitaire_cash_calibration(
    args: &CalibrateSolitaireCashArgs,
) -> Result<(), AdapterError> {
    if !args.image.exists() {
        return Err(AdapterError::CaptureError(format!(
            "sample screenshot does not exist: {}",
            args.image.display()
        )));
    }

    let recognizer = PapayaSolitaireCashRecognizer::from_asset_dir(&args.assets)?;
    let default_calibration = SolitaireCashCalibration::default();
    let session = Arc::new(CalibrationSession::new(
        args.image.clone(),
        recognizer,
        default_calibration,
    ));

    let listener = TcpListener::bind(&args.bind).map_err(|err| {
        AdapterError::ExecutionError(format!("failed to bind calibration server: {err}"))
    })?;
    let addr = listener.local_addr().map_err(|err| {
        AdapterError::ExecutionError(format!("failed to read calibration server address: {err}"))
    })?;
    let url = format!("http://{addr}");

    println!("Solitaire Cash calibration tool listening on {url}");
    println!("Sample screenshot: {}", args.image.display());
    println!("Press Ctrl-C to stop the server.");

    if args.open {
        let _ = open_browser(&url);
    }

    for stream in listener.incoming() {
        let Ok(stream) = stream else {
            continue;
        };
        if let Err(err) = handle_connection(stream, &session) {
            eprintln!("[solitaire-cash-calibration] request error: {err}");
        }
    }

    Ok(())
}

fn open_browser(url: &str) -> Result<(), AdapterError> {
    #[cfg(target_os = "macos")]
    let program = ("open", vec![url]);
    #[cfg(target_os = "linux")]
    let program = ("xdg-open", vec![url]);
    #[cfg(target_os = "windows")]
    let program = ("cmd", vec!["/C", "start", url]);

    let status = std::process::Command::new(program.0)
        .args(&program.1)
        .status()
        .map_err(|err| {
            AdapterError::ExecutionError(format!("failed to open calibration browser: {err}"))
        })?;
    if status.success() {
        Ok(())
    } else {
        Err(AdapterError::ExecutionError(format!(
            "browser open command exited with status {status}"
        )))
    }
}

fn handle_connection(mut stream: TcpStream, session: &Arc<CalibrationSession>) -> Result<(), AdapterError> {
    let request = read_http_request(&mut stream)?;
    let response = match (request.method.as_str(), request.path.as_str()) {
        ("GET", "/") => HttpResponse::html(CALIBRATOR_HTML.as_bytes().to_vec()),
        ("GET", "/api/init") => HttpResponse::json(session.init_response()?)?,
        ("POST", "/api/preview") => {
            let preview = serde_json::from_slice::<PreviewRequest>(&request.body).map_err(|err| {
                AdapterError::ExecutionError(format!("failed to decode preview request: {err}"))
            })?;
            HttpResponse::json(session.preview_response(preview)?)?
        }
        ("GET", "/screenshot") => HttpResponse::binary("image/png", session.screenshot_bytes()?),
        ("GET", path) if path.starts_with("/artifact/") => {
            let key = path.trim_start_matches("/artifact/");
            let artifacts = session.artifacts.lock().map_err(|_| {
                AdapterError::ExecutionError("failed to lock calibration artifacts".into())
            })?;
            if let Some(artifact) = artifacts.get(key) {
                HttpResponse::binary(artifact.content_type, artifact.body.clone())
            } else {
                HttpResponse::not_found()
            }
        }
        _ => HttpResponse::not_found(),
    };
    write_http_response(&mut stream, response)
}

struct HttpRequest {
    method: String,
    path: String,
    body: Vec<u8>,
}

fn read_http_request(stream: &mut TcpStream) -> Result<HttpRequest, AdapterError> {
    let mut reader = BufReader::new(
        stream
            .try_clone()
            .map_err(|err| AdapterError::ExecutionError(format!("failed to clone stream: {err}")))?,
    );

    let mut request_line = String::new();
    reader
        .read_line(&mut request_line)
        .map_err(|err| AdapterError::ExecutionError(format!("failed to read request line: {err}")))?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("/").to_string();

    let mut content_length = 0usize;
    loop {
        let mut line = String::new();
        reader
            .read_line(&mut line)
            .map_err(|err| AdapterError::ExecutionError(format!("failed to read request header: {err}")))?;
        if line == "\r\n" || line.is_empty() {
            break;
        }
        if let Some(value) = line.strip_prefix("Content-Length:") {
            content_length = value.trim().parse::<usize>().unwrap_or(0);
        }
    }

    let mut body = vec![0u8; content_length];
    if content_length > 0 {
        reader
            .read_exact(&mut body)
            .map_err(|err| AdapterError::ExecutionError(format!("failed to read request body: {err}")))?;
    }

    Ok(HttpRequest { method, path, body })
}

struct HttpResponse {
    status: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

impl HttpResponse {
    fn html(body: Vec<u8>) -> Self {
        Self {
            status: "200 OK",
            content_type: "text/html; charset=utf-8",
            body,
        }
    }

    fn json<T: Serialize>(value: T) -> Result<Self, AdapterError> {
        let body = serde_json::to_vec(&value).map_err(|err| {
            AdapterError::ExecutionError(format!("failed to encode calibration response: {err}"))
        })?;
        Ok(Self {
            status: "200 OK",
            content_type: "application/json; charset=utf-8",
            body,
        })
    }

    fn binary(content_type: &'static str, body: Vec<u8>) -> Self {
        Self {
            status: "200 OK",
            content_type,
            body,
        }
    }

    fn not_found() -> Self {
        Self {
            status: "404 Not Found",
            content_type: "text/plain; charset=utf-8",
            body: b"not found".to_vec(),
        }
    }
}

fn write_http_response(stream: &mut TcpStream, response: HttpResponse) -> Result<(), AdapterError> {
    let header = format!(
        "HTTP/1.1 {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    stream.write_all(header.as_bytes()).map_err(|err| {
        AdapterError::ExecutionError(format!("failed to write calibration response header: {err}"))
    })?;
    stream.write_all(&response.body).map_err(|err| {
        AdapterError::ExecutionError(format!("failed to write calibration response body: {err}"))
    })?;
    stream.flush().map_err(|err| {
        AdapterError::ExecutionError(format!("failed to flush calibration response: {err}"))
    })
}

#[cfg(test)]
mod tests {
    use loneadapter::adapters::solitaire_cash::SolitaireCashCalibration;

    use super::CALIBRATOR_HTML;

    #[test]
    fn calibration_export_is_stable_shape() {
        let literal = SolitaireCashCalibration::default().to_rust_literal();
        assert!(literal.contains("SolitaireCashCalibration"));
        assert!(literal.contains("layout: SolitaireCashLayout"));
        assert!(literal.contains("vision: SolitaireCashVisionCalibration"));
        assert!(literal.contains("rank_rect"));
        assert!(literal.contains("stock_tap_point"));
    }

    #[test]
    fn calibrator_html_contains_property_picker_copy() {
        assert!(CALIBRATOR_HTML.contains("Property Picker"));
        assert!(CALIBRATOR_HTML.contains("Column Start X"));
        assert!(CALIBRATOR_HTML.contains("Waste Box Origin"));
        assert!(CALIBRATOR_HTML.contains("Rank Crop Box"));
        assert!(CALIBRATOR_HTML.contains("Focus Active Pile"));
    }
}
