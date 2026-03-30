use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use arrayvec::ArrayVec;
use image::imageops::FilterType;
use image::{DynamicImage, GrayImage};
use lonelybot::card::{Card, N_SUITS};
use lonelybot::deck::{N_DECK_CARDS, N_PILES};
use lonelybot::partial::parse_card;
use lonelybot::standard::PileVec;

use crate::adapter::AdapterError;

use super::solitaire_cash::{
    ObservedBoard, ObservedPile, Point, SolitaireCashBackend, SolitaireCashLayout,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenRegion {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ScreenRegion {
    pub const fn new(x: u32, y: u32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    fn screencapture_arg(self) -> String {
        format!("{},{},{},{}", self.x, self.y, self.width, self.height)
    }

    fn normalize_component(v: f32) -> f64 {
        f64::from(v.clamp(0.0, 1.0))
    }

    pub fn point_to_screen(self, point: Point) -> (f64, f64) {
        let x = f64::from(self.x) + Self::normalize_component(point.x) * f64::from(self.width);
        let y = f64::from(self.y) + Self::normalize_component(point.y) * f64::from(self.height);
        (x, y)
    }
}

#[derive(Debug, Clone, Default)]
pub struct DebugOptions {
    pub enabled: bool,
    pub dump_dir: Option<PathBuf>,
}

impl DebugOptions {
    fn log(&self, message: impl AsRef<str>) {
        if self.enabled {
            eprintln!("[solitaire-cash-debug] {}", message.as_ref());
        }
    }
}

pub trait ScreenshotSource {
    fn capture_png(&mut self) -> Result<PathBuf, AdapterError>;
}

pub trait PngBoardRecognizer {
    fn recognize_board_from_png(
        &mut self,
        png_path: &Path,
        layout: &SolitaireCashLayout,
    ) -> Result<ObservedBoard, AdapterError>;
}

#[derive(Debug, Clone, Copy)]
struct Rect {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl Rect {
    fn from_norm(
        image_width: u32,
        image_height: u32,
        x: f32,
        y: f32,
        width: f32,
        height: f32,
    ) -> Self {
        let x = (x.clamp(0.0, 1.0) * image_width as f32).round() as u32;
        let y = (y.clamp(0.0, 1.0) * image_height as f32).round() as u32;
        let width = (width.clamp(0.0, 1.0) * image_width as f32).round() as u32;
        let height = (height.clamp(0.0, 1.0) * image_height as f32).round() as u32;
        Self {
            x,
            y,
            width: width.max(1),
            height: height.max(1),
        }
    }

}

#[derive(Debug, Clone)]
struct OcrObservation {
    text: String,
    min_x: f32,
    min_y: f32,
}

#[derive(Debug, Clone)]
struct SuitTemplate {
    suit: char,
    mask: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct VisionOcrRunner {
    source_path: PathBuf,
    executable_path: PathBuf,
}

impl Default for VisionOcrRunner {
    fn default() -> Self {
        let base = std::env::temp_dir().join("lonelybot-solitaire-cash-vision-ocr");
        Self {
            source_path: base.with_extension("swift"),
            executable_path: base.with_extension("bin"),
        }
    }
}

impl VisionOcrRunner {
    fn recognize(&self, image_path: &Path) -> Result<Vec<OcrObservation>, AdapterError> {
        self.ensure_compiled()?;
        let output = Command::new(&self.executable_path)
            .arg(image_path)
            .output()
            .map_err(|err| {
                AdapterError::RecognitionError(format!(
                    "failed to launch Vision OCR helper: {err}"
                ))
            })?;

        if !output.status.success() {
            return Err(AdapterError::RecognitionError(format!(
                "Vision OCR helper exited with status {}: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let mut observations = Vec::new();
        for line in String::from_utf8_lossy(&output.stdout).lines() {
            let mut parts = line.split('\t');
            let Some(text) = parts.next() else { continue };
            let Some(min_x) = parts.next() else { continue };
            let Some(min_y) = parts.next() else { continue };
            let Some(_width) = parts.next() else { continue };
            let Some(_height) = parts.next() else { continue };

            observations.push(OcrObservation {
                text: text.to_string(),
                min_x: min_x.parse().unwrap_or(0.0),
                min_y: min_y.parse().unwrap_or(0.0),
            });
        }

        Ok(observations)
    }

    fn ensure_compiled(&self) -> Result<(), AdapterError> {
        if self.executable_path.exists() {
            return Ok(());
        }

        if let Some(parent) = self.source_path.parent() {
            fs::create_dir_all(parent).map_err(|err| {
                AdapterError::RecognitionError(format!(
                    "failed to create OCR helper directory: {err}"
                ))
            })?;
        }

        fs::write(&self.source_path, VISION_OCR_SWIFT).map_err(|err| {
            AdapterError::RecognitionError(format!("failed to write OCR helper source: {err}"))
        })?;

        let output = Command::new("swiftc")
            .arg(&self.source_path)
            .arg("-o")
            .arg(&self.executable_path)
            .output()
            .map_err(|err| {
                AdapterError::RecognitionError(format!(
                    "failed to compile Vision OCR helper: {err}"
                ))
            })?;

        if !output.status.success() {
            return Err(AdapterError::RecognitionError(format!(
                "swiftc failed while compiling Vision OCR helper: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(())
    }
}

const VISION_OCR_SWIFT: &str = r#"import Foundation
import Vision
import CoreGraphics
import ImageIO

let path = CommandLine.arguments[1]
let url = URL(fileURLWithPath: path)
guard let src = CGImageSourceCreateWithURL(url as CFURL, nil),
      let img = CGImageSourceCreateImageAtIndex(src, 0, nil) else {
    fputs("failed to load image\n", stderr)
    exit(2)
}

let req = VNRecognizeTextRequest()
req.recognitionLevel = .accurate
req.usesLanguageCorrection = false
req.minimumTextHeight = 0.02

let handler = VNImageRequestHandler(cgImage: img, options: [:])
try handler.perform([req])

for obs in req.results ?? [] {
    guard let top = obs.topCandidates(1).first else { continue }
    let bb = obs.boundingBox
    let topY = 1.0 - bb.minY - bb.height
    print("\(top.string)\t\(bb.minX)\t\(topY)\t\(bb.width)\t\(bb.height)")
}
"#;

#[derive(Debug, Clone)]
pub struct PapayaSolitaireCashRecognizer {
    suit_templates: Vec<SuitTemplate>,
    ocr: VisionOcrRunner,
    temp_dir: PathBuf,
    next_temp_id: u64,
    debug: DebugOptions,
}

impl PapayaSolitaireCashRecognizer {
    pub fn from_asset_dir(asset_dir: impl AsRef<Path>) -> Result<Self, AdapterError> {
        let asset_dir = asset_dir.as_ref();
        let suit_templates = vec![
            Self::load_template(asset_dir.join("Club.png"), 'C')?,
            Self::load_template(asset_dir.join("Diamond.png"), 'D')?,
            Self::load_template(asset_dir.join("Heart.png"), 'H')?,
            Self::load_template(asset_dir.join("Spade.png"), 'S')?,
        ];

        Ok(Self {
            suit_templates,
            ocr: VisionOcrRunner::default(),
            temp_dir: std::env::temp_dir().join("lonelybot-solitaire-cash-crops"),
            next_temp_id: 0,
            debug: DebugOptions::default(),
        })
    }

    pub fn with_debug(mut self, debug: DebugOptions) -> Self {
        self.debug = debug;
        self
    }

    fn load_template(path: PathBuf, suit: char) -> Result<SuitTemplate, AdapterError> {
        let image = image::open(&path).map_err(|err| {
            AdapterError::RecognitionError(format!(
                "failed to load suit template {}: {err}",
                path.display()
            ))
        })?;
        Ok(SuitTemplate {
            suit,
            mask: Self::binary_mask(image.to_luma8()),
        })
    }

    fn card_rect(
        layout: &SolitaireCashLayout,
        image_width: u32,
        image_height: u32,
        column: usize,
        top: f32,
    ) -> Rect {
        Rect::from_norm(
            image_width,
            image_height,
            layout.column_lefts[column],
            top,
            layout.card_width,
            layout.card_height,
        )
    }

    fn white_ratio(image: &DynamicImage, rect: Rect) -> f32 {
        let crop = image.crop_imm(rect.x, rect.y, rect.width, rect.height).to_rgb8();
        let mut white = 0usize;
        let total = crop.pixels().len().max(1);
        for px in crop.pixels() {
            if px[0] > 220 && px[1] > 220 && px[2] > 220 {
                white += 1;
            }
        }
        white as f32 / total as f32
    }

    fn purple_ratio(image: &DynamicImage, rect: Rect) -> f32 {
        let crop = image.crop_imm(rect.x, rect.y, rect.width, rect.height).to_rgb8();
        let mut purple = 0usize;
        let total = crop.pixels().len().max(1);
        for px in crop.pixels() {
            let r = i32::from(px[0]);
            let g = i32::from(px[1]);
            let b = i32::from(px[2]);
            if b > 90 && r > 70 && b > g + 20 && r > g + 10 {
                purple += 1;
            }
        }
        purple as f32 / total as f32
    }

    fn is_face_up(&self, image: &DynamicImage, rect: Rect) -> bool {
        Self::white_ratio(image, rect) > 0.40
    }

    fn is_face_down(&self, image: &DynamicImage, rect: Rect) -> bool {
        Self::purple_ratio(image, rect) > 0.08
    }

    fn binary_mask(gray: GrayImage) -> Vec<u8> {
        image::imageops::resize(&gray, 32, 32, FilterType::Triangle)
            .pixels()
            .map(|p| u8::from(p[0] < 220))
            .collect()
    }

    fn next_temp_path(&mut self, stem: &str) -> Result<PathBuf, AdapterError> {
        fs::create_dir_all(&self.temp_dir).map_err(|err| {
            AdapterError::RecognitionError(format!("failed to create crop temp dir: {err}"))
        })?;
        self.next_temp_id += 1;
        Ok(self
            .temp_dir
            .join(format!("{stem}-{}-{}.png", std::process::id(), self.next_temp_id)))
    }

    fn ocr_crop(
        &mut self,
        image: &DynamicImage,
        rect: Rect,
        stem: &str,
    ) -> Result<Vec<OcrObservation>, AdapterError> {
        let path = self.next_temp_path(stem)?;
        image
            .crop_imm(rect.x, rect.y, rect.width, rect.height)
            .save(&path)
            .map_err(|err| {
                AdapterError::RecognitionError(format!(
                    "failed to save temporary OCR crop {}: {err}",
                    path.display()
                ))
            })?;
        self.ocr.recognize(&path)
    }

    fn normalize_rank(text: &str) -> Option<&'static str> {
        let upper = text.trim().to_ascii_uppercase();
        match upper.as_str() {
            "A" | "1" => Some("A"),
            "2" => Some("2"),
            "3" => Some("3"),
            "4" => Some("4"),
            "5" => Some("5"),
            "6" => Some("6"),
            "7" => Some("7"),
            "8" => Some("8"),
            "9" => Some("9"),
            "10" | "IO" | "1O" | "LO" => Some("10"),
            "J" => Some("J"),
            "Q" | "OQ" => Some("Q"),
            "K" => Some("K"),
            _ => None,
        }
    }

    fn rank_for_card(&mut self, image: &DynamicImage, rect: Rect) -> Result<Option<&'static str>, AdapterError> {
        let observations = self.ocr_crop(image, rect, "card-rank")?;
        if self.debug.enabled {
            let raw = observations
                .iter()
                .map(|obs| format!("{}@{:.3},{:.3}", obs.text, obs.min_x, obs.min_y))
                .collect::<Vec<_>>()
                .join(", ");
            self.debug.log(format!("rank OCR candidates: {raw}"));
        }
        Ok(observations
            .iter()
            .filter_map(|obs| Self::normalize_rank(&obs.text))
            .next())
    }

    fn largest_dark_component_mask(
        &self,
        image: &DynamicImage,
        rect: Rect,
    ) -> Option<Vec<u8>> {
        let crop = image
            .crop_imm(rect.x, rect.y, rect.width, rect.height)
            .to_luma8();
        let resized = image::imageops::resize(&crop, 48, 48, FilterType::Triangle);
        let mut min_x = 48usize;
        let mut min_y = 48usize;
        let mut max_x = 0usize;
        let mut max_y = 0usize;
        let mut found = false;

        for (x, y, p) in resized.enumerate_pixels() {
            if p[0] < 220 {
                found = true;
                min_x = min_x.min(x as usize);
                min_y = min_y.min(y as usize);
                max_x = max_x.max(x as usize);
                max_y = max_y.max(y as usize);
            }
        }

        if !found {
            return None;
        }

        let sub = image::imageops::crop_imm(
            &resized,
            min_x as u32,
            min_y as u32,
            (max_x - min_x + 1) as u32,
            (max_y - min_y + 1) as u32,
        )
        .to_image();
        Some(Self::binary_mask(sub))
    }

    fn suit_for_card(&self, image: &DynamicImage, rect: Rect) -> Result<Option<char>, AdapterError> {
        let suit_rect = Rect {
            x: rect.x + (rect.width as f32 * 0.20) as u32,
            y: rect.y + (rect.height as f32 * 0.24) as u32,
            width: (rect.width as f32 * 0.36) as u32,
            height: (rect.height as f32 * 0.34) as u32,
        };
        let Some(mask) = self.largest_dark_component_mask(image, suit_rect) else {
            return Ok(None);
        };

        let best = self
            .suit_templates
            .iter()
            .map(|template| {
                let score = mask
                    .iter()
                    .zip(&template.mask)
                    .map(|(a, b)| i32::from(*a != *b))
                    .sum::<i32>();
                (score, template.suit)
            })
            .min_by_key(|(score, _)| *score)
            .map(|(_, suit)| suit);

        if self.debug.enabled {
            self.debug.log(format!(
                "suit match at ({}, {}, {}, {}) => {:?}",
                suit_rect.x, suit_rect.y, suit_rect.width, suit_rect.height, best
            ));
        }

        Ok(best)
    }

    fn recognize_card(&mut self, image: &DynamicImage, rect: Rect) -> Result<Option<Card>, AdapterError> {
        let Some(rank) = self.rank_for_card(image, rect)? else {
            return Ok(None);
        };
        let Some(suit) = self.suit_for_card(image, rect)? else {
            return Ok(None);
        };
        Ok(parse_card(&format!("{rank}{suit}")))
    }

    fn recognize_foundation(
        &mut self,
        image: &DynamicImage,
        layout: &SolitaireCashLayout,
    ) -> Result<[u8; N_SUITS as usize], AdapterError> {
        let mut foundation = [0u8; N_SUITS as usize];
        for suit_slot in 0..N_SUITS as usize {
            let rect = Self::card_rect(
                layout,
                image.width(),
                image.height(),
                suit_slot,
                layout.top_row_top,
            );
            if self.is_face_up(image, rect) {
                if let Some(card) = self.recognize_card(image, rect)? {
                    foundation[card.suit() as usize] = card.rank() + 1;
                }
            }
        }
        Ok(foundation)
    }

    fn recognize_piles(
        &mut self,
        image: &DynamicImage,
        layout: &SolitaireCashLayout,
    ) -> Result<[ObservedPile; N_PILES as usize], AdapterError> {
        core::array::from_fn(|pile_idx| {
            let mut hidden_count = 0u8;
            for hidden_idx in 0..=pile_idx {
                let top = layout.tableau_top + layout.hidden_fan_y * hidden_idx as f32;
                let rect =
                    Self::card_rect(layout, image.width(), image.height(), pile_idx, top);
                if self.is_face_down(image, rect) {
                    hidden_count += 1;
                } else {
                    break;
                }
            }

            let mut cards = PileVec::new();
            for visible_idx in 0..13usize {
                let top = layout.tableau_top
                    + layout.hidden_fan_y * hidden_count as f32
                    + layout.visible_fan_y * visible_idx as f32;
                let rect =
                    Self::card_rect(layout, image.width(), image.height(), pile_idx, top);
                if !self.is_face_up(image, rect) {
                    break;
                }
                if let Ok(Some(card)) = self.recognize_card(image, rect) {
                    cards.push(card);
                } else {
                    break;
                }
            }

            ObservedPile {
                hidden_count,
                cards,
            }
        })
        .pipe(Ok)
    }

    fn recognize_waste(
        &mut self,
        image: &DynamicImage,
        layout: &SolitaireCashLayout,
    ) -> Result<ArrayVec<Card, { N_DECK_CARDS as usize }>, AdapterError> {
        let image_width = image.width();
        let image_height = image.height();
        let card_width = (layout.card_width * image_width as f32).round() as u32;
        let band = Rect::from_norm(
            image_width,
            image_height,
            layout.column_lefts[4] - 0.02,
            layout.top_row_top,
            (layout.column_lefts[6] + layout.card_width) - (layout.column_lefts[4] - 0.02),
            layout.card_height,
        );
        let observations = self.ocr_crop(image, band, "waste-band")?;
        let mut waste = ArrayVec::<Card, { N_DECK_CARDS as usize }>::new();

        let mut rank_obs: Vec<_> = observations
            .into_iter()
            .filter_map(|obs| Self::normalize_rank(&obs.text).map(|rank| (obs, rank)))
            .filter(|(obs, _)| obs.min_y < 0.45)
            .collect();
        rank_obs.sort_by(|a, b| a.0.min_x.partial_cmp(&b.0.min_x).unwrap());

        for (obs, rank) in rank_obs.into_iter().take(3) {
            let card_left = band.x as i32 + (obs.min_x * band.width as f32).round() as i32
                - (card_width as f32 * 0.04) as i32;
            let rect = Rect {
                x: card_left.max(0) as u32,
                y: band.y,
                width: card_width,
                height: (layout.card_height * image_height as f32).round() as u32,
            };
            if let Some(suit) = self.suit_for_card(image, rect)? {
                if let Some(card) = parse_card(&format!("{rank}{suit}")) {
                    waste.push(card);
                }
            }
        }

        Ok(waste)
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}

impl PngBoardRecognizer for PapayaSolitaireCashRecognizer {
    fn recognize_board_from_png(
        &mut self,
        png_path: &Path,
        layout: &SolitaireCashLayout,
    ) -> Result<ObservedBoard, AdapterError> {
        let image = image::open(png_path).map_err(|err| {
            AdapterError::RecognitionError(format!(
                "failed to load screenshot {}: {err}",
                png_path.display()
            ))
        })?;

        let foundation = self.recognize_foundation(&image, layout)?;
        let piles = self.recognize_piles(&image, layout)?;
        let waste = self.recognize_waste(&image, layout)?;
        let stock_rect = Rect::from_norm(
            image.width(),
            image.height(),
            layout.column_lefts[6],
            layout.top_row_top,
            layout.card_width,
            layout.card_height,
        );
        let stock_present = self.is_face_down(&image, stock_rect);

        let board = ObservedBoard {
            foundation,
            piles,
            waste,
            stock_count: 0,
            stock_present,
        };

        if self.debug.enabled {
            let pile_summary = board
                .piles
                .iter()
                .enumerate()
                .map(|(idx, pile)| format!("p{} hidden={} visible={}", idx + 1, pile.hidden_count, pile.cards.len()))
                .collect::<Vec<_>>()
                .join(", ");
            self.debug.log(format!(
                "recognized board foundation={:?} waste_visible={} stock_present={} {pile_summary}",
                board.foundation,
                board.waste.len(),
                board.stock_present
            ));
        }

        Ok(board)
    }
}

pub trait MouseController {
    fn tap_abs(&mut self, _point: (f64, f64)) -> Result<(), AdapterError> {
        Err(AdapterError::ExecutionError(
            "mouse interaction is not enabled".into(),
        ))
    }

    fn drag_abs(
        &mut self,
        _from: (f64, f64),
        _to: (f64, f64),
    ) -> Result<(), AdapterError> {
        Err(AdapterError::ExecutionError(
            "mouse interaction is not enabled".into(),
        ))
    }

    fn can_interact(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ReadOnlyMouse;

impl MouseController for ReadOnlyMouse {}

#[derive(Debug, Clone)]
pub struct ScreenshotVisionBackend<C, V, M = ReadOnlyMouse> {
    capture: C,
    vision: V,
    mouse: M,
    region: ScreenRegion,
    debug: DebugOptions,
}

impl<C, V> ScreenshotVisionBackend<C, V, ReadOnlyMouse> {
    pub fn new(capture: C, vision: V, region: ScreenRegion) -> Self {
        Self {
            capture,
            vision,
            mouse: ReadOnlyMouse,
            region,
            debug: DebugOptions::default(),
        }
    }
}

impl<C, V, M> ScreenshotVisionBackend<C, V, M> {
    pub fn with_mouse<M2>(self, mouse: M2) -> ScreenshotVisionBackend<C, V, M2> {
        ScreenshotVisionBackend {
            capture: self.capture,
            vision: self.vision,
            mouse,
            region: self.region,
            debug: self.debug,
        }
    }

    pub fn with_debug(mut self, debug: DebugOptions) -> Self {
        self.debug = debug;
        self
    }

    pub fn vision(&self) -> &V {
        &self.vision
    }

    pub fn vision_mut(&mut self) -> &mut V {
        &mut self.vision
    }

    pub fn region(&self) -> ScreenRegion {
        self.region
    }
}

impl<C, V, M> SolitaireCashBackend for ScreenshotVisionBackend<C, V, M>
where
    C: ScreenshotSource,
    V: PngBoardRecognizer,
    M: MouseController,
{
    fn observe(&mut self, layout: &SolitaireCashLayout) -> Result<ObservedBoard, AdapterError> {
        let png_path = self.capture.capture_png()?;
        self.debug
            .log(format!("captured screenshot {}", png_path.display()));
        self.vision.recognize_board_from_png(&png_path, layout)
    }

    fn tap(&mut self, point: Point) -> Result<(), AdapterError> {
        self.mouse.tap_abs(self.region.point_to_screen(point))
    }

    fn drag(&mut self, from: Point, to: Point) -> Result<(), AdapterError> {
        self.mouse
            .drag_abs(self.region.point_to_screen(from), self.region.point_to_screen(to))
    }

    fn can_interact(&self) -> bool {
        self.mouse.can_interact()
    }
}

#[derive(Debug, Clone)]
pub struct MacScreenCapture {
    region: ScreenRegion,
    output_path: PathBuf,
    debug: DebugOptions,
    capture_index: usize,
}

impl MacScreenCapture {
    pub fn new(region: ScreenRegion) -> Self {
        let output_path = std::env::temp_dir().join(format!(
            "lonelybot-solitaire-cash-{}-capture.png",
            std::process::id()
        ));
        Self {
            region,
            output_path,
            debug: DebugOptions::default(),
            capture_index: 0,
        }
    }

    pub fn with_output_path(region: ScreenRegion, output_path: PathBuf) -> Self {
        Self {
            region,
            output_path,
            debug: DebugOptions::default(),
            capture_index: 0,
        }
    }

    pub fn with_debug(mut self, debug: DebugOptions) -> Self {
        self.debug = debug;
        self
    }

    fn build_command(&self) -> Command {
        let mut command = Command::new("screencapture");
        command
            .arg("-x")
            .arg(format!("-R{}", self.region.screencapture_arg()))
            .arg(&self.output_path);
        command
    }

    pub fn output_path(&self) -> &Path {
        &self.output_path
    }
}

impl ScreenshotSource for MacScreenCapture {
    fn capture_png(&mut self) -> Result<PathBuf, AdapterError> {
        let status = self.build_command().status().map_err(|err| {
            AdapterError::CaptureError(format!("failed to launch screencapture: {err}"))
        })?;

        if !status.success() {
            return Err(AdapterError::CaptureError(format!(
                "screencapture exited with status {status}"
            )));
        }

        self.capture_index += 1;
        if let Some(dump_dir) = &self.debug.dump_dir {
            fs::create_dir_all(dump_dir).map_err(|err| {
                AdapterError::CaptureError(format!(
                    "failed to create debug dump dir {}: {err}",
                    dump_dir.display()
                ))
            })?;
            let dump_path = dump_dir.join(format!("capture-{:04}.png", self.capture_index));
            fs::copy(&self.output_path, &dump_path).map_err(|err| {
                AdapterError::CaptureError(format!(
                    "failed to copy debug screenshot to {}: {err}",
                    dump_path.display()
                ))
            })?;
            self.debug
                .log(format!("saved debug capture {}", dump_path.display()));
        }

        Ok(self.output_path.clone())
    }
}

#[cfg(target_os = "macos")]
#[derive(Debug, Clone)]
pub struct MacNativeMouse {
    click_hold: Duration,
    drag_step_delay: Duration,
    drag_steps: u16,
}

#[cfg(target_os = "macos")]
impl Default for MacNativeMouse {
    fn default() -> Self {
        Self {
            click_hold: Duration::from_millis(25),
            drag_step_delay: Duration::from_millis(8),
            drag_steps: 10,
        }
    }
}

#[cfg(target_os = "macos")]
impl MacNativeMouse {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_drag_steps(mut self, drag_steps: u16) -> Self {
        self.drag_steps = drag_steps.max(1);
        self
    }
}

#[cfg(target_os = "macos")]
impl MouseController for MacNativeMouse {
    fn tap_abs(&mut self, point: (f64, f64)) -> Result<(), AdapterError> {
        post_mouse_event(MouseEventKind::Moved, point)?;
        post_mouse_event(MouseEventKind::LeftDown, point)?;
        thread::sleep(self.click_hold);
        post_mouse_event(MouseEventKind::LeftUp, point)
    }

    fn drag_abs(&mut self, from: (f64, f64), to: (f64, f64)) -> Result<(), AdapterError> {
        post_mouse_event(MouseEventKind::Moved, from)?;
        post_mouse_event(MouseEventKind::LeftDown, from)?;

        for step in 1..=self.drag_steps {
            let progress = f64::from(step) / f64::from(self.drag_steps);
            let point = (
                from.0 + (to.0 - from.0) * progress,
                from.1 + (to.1 - from.1) * progress,
            );
            post_mouse_event(MouseEventKind::LeftDragged, point)?;
            thread::sleep(self.drag_step_delay);
        }

        post_mouse_event(MouseEventKind::LeftUp, to)
    }

    fn can_interact(&self) -> bool {
        true
    }
}

#[cfg(not(target_os = "macos"))]
#[derive(Debug, Clone, Copy, Default)]
pub struct MacNativeMouse;

#[cfg(not(target_os = "macos"))]
impl MacNativeMouse {
    pub fn new() -> Self {
        Self
    }
}

#[cfg(not(target_os = "macos"))]
impl MouseController for MacNativeMouse {}

#[cfg(target_os = "macos")]
mod macos_mouse {
    use std::ffi::c_void;

    use crate::adapter::AdapterError;

    #[repr(C)]
    #[derive(Debug, Clone, Copy)]
    pub struct CGPoint {
        pub x: f64,
        pub y: f64,
    }

    #[derive(Debug, Clone, Copy)]
    pub enum MouseEventKind {
        Moved = 5,
        LeftDown = 1,
        LeftUp = 2,
        LeftDragged = 6,
    }

    struct OwnedEvent(*mut c_void);

    impl Drop for OwnedEvent {
        fn drop(&mut self) {
            unsafe { CFRelease(self.0) };
        }
    }

    #[link(name = "ApplicationServices", kind = "framework")]
    unsafe extern "C" {
        fn CGEventCreateMouseEvent(
            source: *const c_void,
            mouse_type: u32,
            mouse_cursor_position: CGPoint,
            mouse_button: u32,
        ) -> *mut c_void;

        fn CGEventPost(tap: u32, event: *mut c_void);
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        fn CFRelease(cf: *const c_void);
    }

    pub fn post_mouse_event(
        kind: MouseEventKind,
        point: (f64, f64),
    ) -> Result<(), AdapterError> {
        let event = unsafe {
            CGEventCreateMouseEvent(
                std::ptr::null(),
                kind as u32,
                CGPoint {
                    x: point.0,
                    y: point.1,
                },
                0,
            )
        };

        if event.is_null() {
            return Err(AdapterError::ExecutionError(
                "failed to create macOS mouse event".into(),
            ));
        }

        let event = OwnedEvent(event);
        unsafe { CGEventPost(0, event.0) };
        Ok(())
    }
}

#[cfg(target_os = "macos")]
use macos_mouse::{post_mouse_event, MouseEventKind};

#[cfg(test)]
mod tests {
    use super::*;
    use arrayvec::ArrayVec;
    use lonelybot::card::Card;
    use lonelybot::deck::N_DECK_CARDS;
    use lonelybot::standard::PileVec;

    #[derive(Debug, Clone)]
    struct MockCapture {
        next_path: PathBuf,
        calls: usize,
    }

    impl ScreenshotSource for MockCapture {
        fn capture_png(&mut self) -> Result<PathBuf, AdapterError> {
            self.calls += 1;
            Ok(self.next_path.clone())
        }
    }

    #[derive(Debug, Clone)]
    struct MockVision {
        seen_paths: Vec<PathBuf>,
        board: ObservedBoard,
    }

    impl PngBoardRecognizer for MockVision {
        fn recognize_board_from_png(
            &mut self,
            png_path: &Path,
            _layout: &SolitaireCashLayout,
        ) -> Result<ObservedBoard, AdapterError> {
            self.seen_paths.push(png_path.to_path_buf());
            Ok(self.board.clone())
        }
    }

    #[derive(Debug, Clone, Default)]
    struct MockMouse {
        taps: Vec<(f64, f64)>,
        drags: Vec<((f64, f64), (f64, f64))>,
    }

    impl MouseController for MockMouse {
        fn tap_abs(&mut self, point: (f64, f64)) -> Result<(), AdapterError> {
            self.taps.push(point);
            Ok(())
        }

        fn drag_abs(
            &mut self,
            from: (f64, f64),
            to: (f64, f64),
        ) -> Result<(), AdapterError> {
            self.drags.push((from, to));
            Ok(())
        }

        fn can_interact(&self) -> bool {
            true
        }
    }

    fn empty_board() -> ObservedBoard {
        ObservedBoard {
            foundation: [13; 4],
            piles: core::array::from_fn(|_| super::super::solitaire_cash::ObservedPile {
                hidden_count: 0,
                cards: PileVec::new(),
            }),
            waste: ArrayVec::<Card, { N_DECK_CARDS as usize }>::new(),
            stock_count: 0,
            stock_present: false,
        }
    }

    #[test]
    fn region_maps_normalized_points_to_screen_coordinates() {
        let region = ScreenRegion::new(100, 200, 800, 1600);
        let point = region.point_to_screen(Point { x: 0.25, y: 0.75 });
        assert_eq!(point, (300.0, 1400.0));
    }

    #[test]
    fn screenshot_backend_observes_via_capture_then_vision() {
        let capture_path = PathBuf::from("/tmp/solitaire-cash-test.png");
        let capture = MockCapture {
            next_path: capture_path.clone(),
            calls: 0,
        };
        let vision = MockVision {
            seen_paths: Vec::new(),
            board: empty_board(),
        };
        let mut backend =
            ScreenshotVisionBackend::new(capture, vision, ScreenRegion::new(0, 0, 100, 200));

        let observed = backend.observe(&SolitaireCashLayout::default()).unwrap();

        assert_eq!(observed.stock_count, 0);
        assert_eq!(backend.capture.calls, 1);
        assert_eq!(backend.vision.seen_paths, vec![capture_path]);
    }

    #[test]
    fn screenshot_backend_maps_taps_and_drags_through_region() {
        let capture = MockCapture {
            next_path: PathBuf::from("/tmp/ignored.png"),
            calls: 0,
        };
        let vision = MockVision {
            seen_paths: Vec::new(),
            board: empty_board(),
        };
        let mouse = MockMouse::default();
        let mut backend = ScreenshotVisionBackend::new(
            capture,
            vision,
            ScreenRegion::new(50, 75, 200, 400),
        )
        .with_mouse(mouse);

        backend.tap(Point { x: 0.5, y: 0.25 }).unwrap();
        backend
            .drag(Point { x: 0.0, y: 0.0 }, Point { x: 1.0, y: 1.0 })
            .unwrap();

        assert_eq!(backend.mouse.taps, vec![(150.0, 175.0)]);
        assert_eq!(backend.mouse.drags, vec![((50.0, 75.0), (250.0, 475.0))]);
        assert!(backend.can_interact());
    }

    #[test]
    fn mac_screen_capture_builds_expected_region_flag() {
        let capture = MacScreenCapture::with_output_path(
            ScreenRegion::new(12, 34, 560, 780),
            PathBuf::from("/tmp/capture.png"),
        );
        let command = capture.build_command();
        let args: Vec<_> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            args,
            vec![
                "-x".to_string(),
                "-R12,34,560,780".to_string(),
                "/tmp/capture.png".to_string()
            ]
        );
    }
}
