use std::fs;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::Duration;

use arrayvec::ArrayVec;
use image::{DynamicImage, GrayImage, ImageFormat, Rgba, RgbaImage};
use lonelybot::card::{Card, N_SUITS};
use lonelybot::deck::N_DECK_CARDS;
use lonelybot::standard::PileVec;

use crate::adapter::AdapterError;

use super::solitaire_cash::{
    NormalizedRect, ObservedBoard, ObservedPile, Point, SolitaireCashBackend,
    SolitaireCashCalibration, SolitaireCashLayout,
};
use super::solitaire_cash_templates::{MatchCandidate, MatchReport, TemplateLibrary};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotState {
    Empty,
    FaceDown,
    FaceUp,
    Recycle,
}

#[derive(Debug, Clone)]
pub struct SlotReport {
    pub label: String,
    pub rect: NormalizedRect,
    pub state: SlotState,
    pub card: Option<Card>,
    pub rank_candidates: Vec<MatchCandidate>,
    pub suit_candidates: Vec<MatchCandidate>,
    pub low_confidence: bool,
}

#[derive(Debug, Clone)]
pub struct RecognitionReport {
    pub board: ObservedBoard,
    pub slots: Vec<SlotReport>,
    pub annotated_path: Option<PathBuf>,
    pub image_width: u32,
    pub image_height: u32,
}

#[derive(Debug, Clone)]
pub struct SelectedSlotPreview {
    pub label: String,
    pub face_rect: NormalizedRect,
    pub rank_rect: NormalizedRect,
    pub suit_rect: NormalizedRect,
    pub rank_candidates: Vec<MatchCandidate>,
    pub suit_candidates: Vec<MatchCandidate>,
    pub rank_raw_png: Vec<u8>,
    pub suit_raw_png: Vec<u8>,
    pub rank_mask_png: Vec<u8>,
    pub suit_mask_png: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct CalibrationPreview {
    pub report: RecognitionReport,
    pub selected_slot: Option<SelectedSlotPreview>,
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

    fn to_normalized(self, image_width: u32, image_height: u32) -> NormalizedRect {
        NormalizedRect::new(
            self.x as f32 / image_width.max(1) as f32,
            self.y as f32 / image_height.max(1) as f32,
            self.width as f32 / image_width.max(1) as f32,
            self.height as f32 / image_height.max(1) as f32,
        )
    }

    fn from_normalized(image_width: u32, image_height: u32, rect: NormalizedRect) -> Self {
        Self::from_norm(
            image_width,
            image_height,
            rect.x,
            rect.y,
            rect.width,
            rect.height,
        )
    }
}

#[derive(Debug, Clone)]
pub struct PapayaSolitaireCashRecognizer {
    calibration: SolitaireCashCalibration,
    templates: TemplateLibrary,
    next_debug_id: u64,
    debug: DebugOptions,
}

#[derive(Debug, Clone)]
struct CardRecognition {
    card: Option<Card>,
    rank: Option<MatchReport>,
    suit: Option<MatchReport>,
    low_confidence: bool,
}

#[derive(Debug, Clone, Copy)]
struct CardCornerSignal {
    accepted_pair: bool,
    rank_best: f32,
    suit_best: f32,
}

impl PapayaSolitaireCashRecognizer {
    pub fn from_asset_dir(asset_dir: impl AsRef<Path>) -> Result<Self, AdapterError> {
        let asset_dir = asset_dir.as_ref();
        Ok(Self {
            calibration: SolitaireCashCalibration::default(),
            templates: TemplateLibrary::load(asset_dir)?,
            next_debug_id: 0,
            debug: DebugOptions::default(),
        })
    }

    pub fn with_calibration(mut self, calibration: SolitaireCashCalibration) -> Self {
        self.calibration = calibration;
        self
    }

    pub fn with_debug(mut self, debug: DebugOptions) -> Self {
        self.debug = debug;
        self
    }

    fn apply_relative_rect(rect: Rect, crop: NormalizedRect) -> Rect {
        Rect {
            x: rect.x + (rect.width as f32 * crop.x) as u32,
            y: rect.y + (rect.height as f32 * crop.y) as u32,
            width: (rect.width as f32 * crop.width).max(1.0) as u32,
            height: (rect.height as f32 * crop.height).max(1.0) as u32,
        }
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

    fn white_ratio(&self, image: &DynamicImage, rect: Rect) -> f32 {
        let crop = image.crop_imm(rect.x, rect.y, rect.width, rect.height).to_rgb8();
        let mut white = 0usize;
        let total = crop.pixels().len().max(1);
        let threshold = self.calibration.vision.white_min_rgb;
        for px in crop.pixels() {
            if px[0] > threshold && px[1] > threshold && px[2] > threshold {
                white += 1;
            }
        }
        white as f32 / total as f32
    }

    fn ink_ratio(&self, image: &DynamicImage, rect: Rect) -> f32 {
        let crop = image.crop_imm(rect.x, rect.y, rect.width, rect.height).to_rgb8();
        let mut ink = 0usize;
        let total = crop.pixels().len().max(1);
        let vision = self.calibration.vision;
        for px in crop.pixels() {
            let is_white = px[0] > vision.white_min_rgb
                && px[1] > vision.white_min_rgb
                && px[2] > vision.white_min_rgb;
            let distance_from_background =
                (i32::from(px[0]) - i32::from(vision.background_rgb[0])).abs()
                    + (i32::from(px[1]) - i32::from(vision.background_rgb[1])).abs()
                    + (i32::from(px[2]) - i32::from(vision.background_rgb[2])).abs();
            if !is_white && distance_from_background > vision.background_distance_threshold {
                ink += 1;
            }
        }
        ink as f32 / total as f32
    }

    fn purple_ratio(&self, image: &DynamicImage, rect: Rect) -> f32 {
        let crop = image.crop_imm(rect.x, rect.y, rect.width, rect.height).to_rgb8();
        let mut purple = 0usize;
        let total = crop.pixels().len().max(1);
        let vision = self.calibration.vision;
        for px in crop.pixels() {
            let r = i32::from(px[0]);
            let g = i32::from(px[1]);
            let b = i32::from(px[2]);
            if b > vision.purple_blue_min.into()
                && r > vision.purple_red_min.into()
                && b > g + vision.purple_blue_over_green
                && r > g + vision.purple_red_over_green
            {
                purple += 1;
            }
        }
        purple as f32 / total as f32
    }

    fn is_face_up(&self, image: &DynamicImage, rect: Rect) -> bool {
        let corner = self.top_left_probe_rect(rect, 0.24, 0.18);
        self.face_up_probe_passes(image, corner)
    }

    fn is_face_down(&self, image: &DynamicImage, rect: Rect) -> bool {
        let top_band = self.top_strip_probe_rect(rect, 0.22);
        self.purple_ratio(image, top_band) > self.calibration.vision.face_down_purple_ratio
    }

    fn top_left_probe_rect(&self, rect: Rect, width_fraction: f32, height_fraction: f32) -> Rect {
        Rect {
            x: rect.x,
            y: rect.y,
            width: (rect.width as f32 * width_fraction).max(1.0) as u32,
            height: (rect.height as f32 * height_fraction).max(1.0) as u32,
        }
    }

    fn top_strip_probe_rect(&self, rect: Rect, height_fraction: f32) -> Rect {
        Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width.max(1),
            height: (rect.height as f32 * height_fraction).max(1.0) as u32,
        }
    }

    fn exposed_strip_rect(&self, rect: Rect, exposed_height: u32) -> Rect {
        Rect {
            x: rect.x,
            y: rect.y,
            width: rect.width.max(1),
            height: exposed_height.clamp(1, rect.height.max(1)),
        }
    }

    fn face_up_probe_passes(&self, image: &DynamicImage, rect: Rect) -> bool {
        self.white_ratio(image, rect) > self.calibration.vision.face_up_white_ratio
            && self.ink_ratio(image, rect) > 0.04
    }

    fn normalized_y_to_pixels(&self, normalized_y: f32, image_height: u32, cap: u32) -> u32 {
        ((normalized_y * image_height as f32).round() as u32)
            .max(1)
            .min(cap.max(1))
    }

    fn classify_tableau_hidden_state(
        &self,
        image: &DynamicImage,
        rect: Rect,
        layout: &SolitaireCashLayout,
        image_height: u32,
        label: &str,
    ) -> SlotState {
        let exposed = self.normalized_y_to_pixels(layout.hidden_fan_y, image_height, rect.height);
        let strip = self.exposed_strip_rect(rect, exposed);
        let face_probe = self.top_left_probe_rect(strip, 0.34, 1.0);
        let purple = self.purple_ratio(image, strip);
        let white = self.white_ratio(image, face_probe);
        let ink = self.ink_ratio(image, face_probe);
        let corner_signal = self.quick_card_corner_signal(image, rect).ok();
        let corner_match = corner_signal.is_some_and(|signal| {
            signal.accepted_pair || (signal.rank_best < 0.12 && signal.suit_best < 0.12)
        });
        let state = if purple > self.calibration.vision.face_down_purple_ratio && white < 0.55 {
            SlotState::FaceDown
        } else if white > self.calibration.vision.face_up_white_ratio
            && ink > 0.04
            && corner_match
        {
            SlotState::FaceUp
        } else {
            SlotState::Empty
        };
        if self.debug.enabled {
            let corner_summary = corner_signal.map_or_else(
                || "corner=none".to_string(),
                |signal| {
                    format!(
                        "corner_match={} accepted_pair={} rank_best={:.3} suit_best={:.3}",
                        corner_match, signal.accepted_pair, signal.rank_best, signal.suit_best
                    )
                },
            );
            self.debug.log(format!(
                "{label} hidden-strip purple={purple:.3} white={white:.3} ink={ink:.3} {corner_summary} -> {state:?}"
            ));
        }
        state
    }

    fn classify_tableau_visible_state(
        &self,
        image: &DynamicImage,
        rect: Rect,
        layout: &SolitaireCashLayout,
        image_height: u32,
        label: &str,
    ) -> SlotState {
        let exposed = self.normalized_y_to_pixels(layout.visible_fan_y, image_height, rect.height);
        let strip = self.exposed_strip_rect(rect, exposed);
        let face_probe = self.top_left_probe_rect(strip, 0.38, 1.0);
        let purple = self.purple_ratio(image, strip);
        let white = self.white_ratio(image, face_probe);
        let ink = self.ink_ratio(image, face_probe);
        let corner_signal = self.quick_card_corner_signal(image, rect).ok();
        let corner_match = corner_signal.is_some_and(|signal| {
            signal.accepted_pair || (signal.rank_best < 0.12 && signal.suit_best < 0.12)
        });
        let state = if purple < self.calibration.vision.face_down_purple_ratio
            && white > 0.32
            && ink > 0.035
            && corner_match
        {
            SlotState::FaceUp
        } else {
            SlotState::Empty
        };
        if self.debug.enabled {
            let corner_summary = corner_signal.map_or_else(
                || "corner=none".to_string(),
                |signal| {
                    format!(
                        "corner_match={} accepted_pair={} rank_best={:.3} suit_best={:.3}",
                        corner_match, signal.accepted_pair, signal.rank_best, signal.suit_best
                    )
                },
            );
            self.debug.log(format!(
                "{label} visible-strip purple={purple:.3} white={white:.3} ink={ink:.3} {corner_summary} -> {state:?}"
            ));
        }
        state
    }

    fn next_debug_path(&mut self, stem: &str, suffix: &str) -> Result<Option<PathBuf>, AdapterError> {
        let Some(dump_dir) = &self.debug.dump_dir else {
            return Ok(None);
        };
        fs::create_dir_all(dump_dir).map_err(|err| {
            AdapterError::RecognitionError(format!(
                "failed to create debug crop dir {}: {err}",
                dump_dir.display()
            ))
        })?;
        self.next_debug_id += 1;
        Ok(Some(dump_dir.join(format!(
            "{stem}-{}-{}.{}",
            std::process::id(),
            self.next_debug_id,
            suffix
        ))))
    }

    fn save_debug_crop(
        &mut self,
        image: &DynamicImage,
        rect: Rect,
        stem: &str,
    ) -> Result<(), AdapterError> {
        let Some(path) = self.next_debug_path(stem, "png")? else {
            return Ok(());
        };
        image
            .crop_imm(rect.x, rect.y, rect.width, rect.height)
            .save(&path)
            .map_err(|err| {
                AdapterError::RecognitionError(format!(
                    "failed to save debug crop {}: {err}",
                    path.display()
                ))
            })?;
        self.debug.log(format!("saved debug crop {}", path.display()));
        Ok(())
    }

    fn save_debug_mask(&mut self, mask: &GrayImage, stem: &str) -> Result<(), AdapterError> {
        let Some(path) = self.next_debug_path(stem, "png")? else {
            return Ok(());
        };
        mask
            .save(&path)
            .map_err(|err| {
                AdapterError::RecognitionError(format!(
                    "failed to save debug mask {}: {err}",
                    path.display()
                ))
            })?;
        self.debug.log(format!("saved debug mask {}", path.display()));
        Ok(())
    }

    fn debug_slot_stem(&self, slot_label: &str, suffix: &str) -> String {
        let mut sanitized = String::with_capacity(slot_label.len() + suffix.len() + 1);
        for ch in slot_label.chars() {
            if ch.is_ascii_alphanumeric() {
                sanitized.push(ch);
            } else {
                sanitized.push('_');
            }
        }
        sanitized.push('-');
        sanitized.push_str(suffix);
        sanitized
    }

    fn locate_face_anchor(&self, image: &DynamicImage, rect: Rect) -> Rect {
        let crop = image.crop_imm(rect.x, rect.y, rect.width, rect.height).to_rgb8();
        let mut left = 0u32;
        let mut top = 0u32;
        let vision = self.calibration.vision;

        for x in 0..crop.width() {
            let bright = (0..crop.height())
                .filter(|y| {
                    let px = crop.get_pixel(x, *y);
                    px[0] > vision.white_min_rgb
                        && px[1] > vision.white_min_rgb
                        && px[2] > vision.white_min_rgb
                })
                .count();
            if bright as f32 / crop.height().max(1) as f32 > vision.face_anchor_bright_ratio {
                left = x.saturating_sub(vision.face_anchor_padding_px);
                break;
            }
        }
        for y in 0..crop.height() {
            let bright = (0..crop.width())
                .filter(|x| {
                    let px = crop.get_pixel(*x, y);
                    px[0] > vision.white_min_rgb
                        && px[1] > vision.white_min_rgb
                        && px[2] > vision.white_min_rgb
                })
                .count();
            if bright as f32 / crop.width().max(1) as f32 > vision.face_anchor_bright_ratio {
                top = y.saturating_sub(vision.face_anchor_padding_px);
                break;
            }
        }

        Rect {
            x: rect.x + left,
            y: rect.y + top,
            width: rect.width.saturating_sub(left).max(1),
            height: rect.height.saturating_sub(top).max(1),
        }
    }

    fn grayscale_crop(&self, image: &DynamicImage, rect: Rect) -> GrayImage {
        image
            .crop_imm(rect.x, rect.y, rect.width, rect.height)
            .to_luma8()
    }

    fn image_to_png_bytes(&self, image: &DynamicImage) -> Result<Vec<u8>, AdapterError> {
        let mut out = Cursor::new(Vec::new());
        image.write_to(&mut out, ImageFormat::Png).map_err(|err| {
            AdapterError::RecognitionError(format!("failed to encode preview png: {err}"))
        })?;
        Ok(out.into_inner())
    }

    fn gray_to_png_bytes(&self, image: &GrayImage) -> Result<Vec<u8>, AdapterError> {
        self.image_to_png_bytes(&DynamicImage::ImageLuma8(image.clone()))
    }

    fn rank_rect(&self, rect: Rect) -> Rect {
        Self::apply_relative_rect(rect, self.calibration.vision.rank_rect)
    }

    fn suit_rect(&self, rect: Rect) -> Rect {
        Self::apply_relative_rect(rect, self.calibration.vision.suit_rect)
    }

    fn quick_card_corner_signal(
        &self,
        image: &DynamicImage,
        rect: Rect,
    ) -> Result<CardCornerSignal, AdapterError> {
        let anchored = self.locate_face_anchor(image, rect);
        let rank = self
            .templates
            .match_rank(&self.grayscale_crop(image, self.rank_rect(anchored)))?;
        let suit = self
            .templates
            .match_suit(&self.grayscale_crop(image, self.suit_rect(anchored)))?;
        Ok(CardCornerSignal {
            accepted_pair: rank.accepted.is_some() && suit.accepted.is_some(),
            rank_best: rank.candidates.first().map_or(1.0, |candidate| candidate.score),
            suit_best: suit.candidates.first().map_or(1.0, |candidate| candidate.score),
        })
    }

    fn match_rank_report(
        &mut self,
        image: &DynamicImage,
        rect: Rect,
        slot_label: &str,
    ) -> Result<MatchReport, AdapterError> {
        let rank_rect = self.rank_rect(self.locate_face_anchor(image, rect));
        self.save_debug_crop(image, rank_rect, &self.debug_slot_stem(slot_label, "rank-raw"))?;
        let gray = self.grayscale_crop(image, rank_rect);
        let report = self.templates.match_rank(&gray)?;
        self.save_debug_mask(
            &report.normalized_mask,
            &self.debug_slot_stem(slot_label, "rank-mask"),
        )?;
        if self.debug.enabled {
            self.debug.log(format!(
                "{slot_label} rank candidates: {}",
                report
                    .candidates
                    .iter()
                    .take(3)
                    .map(|candidate| format!("{}:{:.3}", candidate.label, candidate.score))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(report)
    }

    fn match_suit_report(
        &mut self,
        image: &DynamicImage,
        rect: Rect,
        slot_label: &str,
    ) -> Result<MatchReport, AdapterError> {
        let suit_rect = self.suit_rect(self.locate_face_anchor(image, rect));
        self.save_debug_crop(image, suit_rect, &self.debug_slot_stem(slot_label, "suit-raw"))?;
        let gray = self.grayscale_crop(image, suit_rect);
        let report = self.templates.match_suit(&gray)?;
        self.save_debug_mask(
            &report.normalized_mask,
            &self.debug_slot_stem(slot_label, "suit-mask"),
        )?;
        if self.debug.enabled {
            self.debug.log(format!(
                "{slot_label} suit candidates: {}",
                report
                    .candidates
                    .iter()
                    .take(3)
                    .map(|candidate| format!("{}:{:.3}", candidate.label, candidate.score))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        Ok(report)
    }

    fn recognize_card(
        &mut self,
        image: &DynamicImage,
        rect: Rect,
        slot_label: &str,
    ) -> Result<CardRecognition, AdapterError> {
        let rank = self.match_rank_report(image, rect, slot_label)?;
        let suit = self.match_suit_report(image, rect, slot_label)?;
        let low_confidence = rank.accepted.is_none() || suit.accepted.is_none();
        let card = rank
            .accepted
            .as_deref()
            .zip(suit.accepted.as_deref())
            .and_then(|(rank, suit)| lonelybot::partial::parse_card(&format!("{rank}{suit}")));
        Ok(CardRecognition {
            card,
            rank: Some(rank),
            suit: Some(suit),
            low_confidence,
        })
    }

    fn classify_general_state(&self, image: &DynamicImage, rect: Rect) -> SlotState {
        if self.is_face_up(image, rect) {
            SlotState::FaceUp
        } else if self.is_face_down(image, rect) {
            SlotState::FaceDown
        } else {
            SlotState::Empty
        }
    }

    fn center_foreground_ratio(&self, image: &DynamicImage, rect: Rect) -> f32 {
        let vision = self.calibration.vision;
        let inset_x = (rect.width as f32 * vision.center_foreground_inset_x) as u32;
        let inset_y = (rect.height as f32 * vision.center_foreground_inset_y) as u32;
        let inner = Rect {
            x: rect.x + inset_x,
            y: rect.y + inset_y,
            width: rect.width.saturating_sub(inset_x * 2).max(1),
            height: rect.height.saturating_sub(inset_y * 2).max(1),
        };
        let crop = image.crop_imm(inner.x, inner.y, inner.width, inner.height).to_rgb8();
        let mut foreground = 0usize;
        let total = crop.pixels().len().max(1);
        for px in crop.pixels() {
            let r = i32::from(px[0]);
            let g = i32::from(px[1]);
            let b = i32::from(px[2]);
            if (r - i32::from(vision.background_rgb[0])).abs()
                + (g - i32::from(vision.background_rgb[1])).abs()
                + (b - i32::from(vision.background_rgb[2])).abs()
                > vision.background_distance_threshold
            {
                foreground += 1;
            }
        }
        foreground as f32 / total as f32
    }

    fn classify_stock_state(&self, image: &DynamicImage, rect: Rect) -> SlotState {
        if self.is_face_down(image, rect) {
            SlotState::FaceDown
        } else if self.center_foreground_ratio(image, rect)
            > self.calibration.vision.recycle_foreground_ratio
        {
            SlotState::Recycle
        } else {
            SlotState::Empty
        }
    }

    fn record_slot(
        &self,
        reports: &mut Vec<SlotReport>,
        label: impl Into<String>,
        rect: Rect,
        image_width: u32,
        image_height: u32,
        state: SlotState,
        recognition: Option<CardRecognition>,
    ) {
        let (card, rank_candidates, suit_candidates, low_confidence) = recognition
            .map(|recognition| {
                (
                    recognition.card,
                    recognition.rank.map_or_else(Vec::new, |report| report.candidates),
                    recognition.suit.map_or_else(Vec::new, |report| report.candidates),
                    recognition.low_confidence,
                )
            })
            .unwrap_or_else(|| (None, Vec::new(), Vec::new(), false));
        reports.push(SlotReport {
            label: label.into(),
            rect: rect.to_normalized(image_width, image_height),
            state,
            card,
            rank_candidates,
            suit_candidates,
            low_confidence,
        });
    }

    fn waste_rects(&self, layout: &SolitaireCashLayout, image_width: u32, image_height: u32) -> [Rect; 3] {
        let overlap = layout.card_width * self.calibration.vision.waste_overlap;
        core::array::from_fn(|idx| {
            Rect::from_norm(
                image_width,
                image_height,
                layout.waste_origin.x + overlap * idx as f32,
                layout.waste_origin.y,
                layout.card_width,
                layout.card_height,
            )
        })
    }

    fn draw_rect_outline(image: &mut RgbaImage, rect: Rect, color: Rgba<u8>) {
        let max_x = image.width().saturating_sub(1);
        let max_y = image.height().saturating_sub(1);
        let left = rect.x.min(max_x);
        let top = rect.y.min(max_y);
        let right = rect.x.saturating_add(rect.width).saturating_sub(1).min(max_x);
        let bottom = rect.y.saturating_add(rect.height).saturating_sub(1).min(max_y);
        for x in left..=right {
            image.put_pixel(x, top, color);
            image.put_pixel(x, bottom, color);
        }
        for y in top..=bottom {
            image.put_pixel(left, y, color);
            image.put_pixel(right, y, color);
        }
    }

    fn state_color(state: SlotState) -> Rgba<u8> {
        match state {
            SlotState::Empty => Rgba([120, 180, 120, 255]),
            SlotState::FaceDown => Rgba([140, 90, 220, 255]),
            SlotState::FaceUp => Rgba([255, 255, 255, 255]),
            SlotState::Recycle => Rgba([240, 180, 80, 255]),
        }
    }

    fn save_annotated_overlay(
        &mut self,
        image: &DynamicImage,
        rects: &[(Rect, SlotState)],
        png_path: &Path,
    ) -> Result<Option<PathBuf>, AdapterError> {
        let Some(path) = self.next_debug_path(
            png_path.file_stem().and_then(|stem| stem.to_str()).unwrap_or("annotated"),
            "png",
        )?
        else {
            return Ok(None);
        };
        let mut annotated = image.to_rgba8();
        for (rect, state) in rects {
            Self::draw_rect_outline(&mut annotated, *rect, Self::state_color(*state));
        }
        annotated.save(&path).map_err(|err| {
            AdapterError::RecognitionError(format!(
                "failed to save annotated overlay {}: {err}",
                path.display()
            ))
        })?;
        self.debug.log(format!("saved annotated overlay {}", path.display()));
        Ok(Some(path))
    }

    pub fn inspect_png(
        &mut self,
        png_path: &Path,
        layout: &SolitaireCashLayout,
    ) -> Result<RecognitionReport, AdapterError> {
        let image = image::open(png_path).map_err(|err| {
            AdapterError::RecognitionError(format!(
                "failed to load screenshot {}: {err}",
                png_path.display()
            ))
        })?;

        let mut slot_reports = Vec::new();
        let mut overlay_rects = Vec::new();
        let mut foundation = [0u8; N_SUITS as usize];
        for suit_slot in 0..N_SUITS as usize {
            let label = format!("foundation-{}", suit_slot + 1);
            let rect = Self::card_rect(layout, image.width(), image.height(), suit_slot, layout.top_row_top);
            let state = self.classify_general_state(&image, rect);
            overlay_rects.push((rect, state));
            let recognition = if state == SlotState::FaceUp {
                Some(self.recognize_card(&image, rect, &label)?)
            } else {
                None
            };
            if let Some(card) = recognition.as_ref().and_then(|result| result.card) {
                foundation[card.suit() as usize] = card.rank() + 1;
            }
            self.record_slot(
                &mut slot_reports,
                label,
                rect,
                image.width(),
                image.height(),
                state,
                recognition,
            );
        }

        let piles = core::array::from_fn(|pile_idx| {
            let mut hidden_count = 0u8;
            for hidden_idx in 0..=pile_idx {
                let label = format!("pile-{}-hidden-{}", pile_idx + 1, hidden_idx + 1);
                let rect = Self::card_rect(
                    layout,
                    image.width(),
                    image.height(),
                    pile_idx,
                    layout.tableau_top + layout.hidden_fan_y * hidden_idx as f32,
                );
                let state = self.classify_tableau_hidden_state(
                    &image,
                    rect,
                    layout,
                    image.height(),
                    &label,
                );
                overlay_rects.push((rect, state));
                match state {
                    SlotState::FaceDown => {
                        hidden_count += 1;
                        self.record_slot(
                            &mut slot_reports,
                            label,
                            rect,
                            image.width(),
                            image.height(),
                            state,
                            None,
                        );
                    }
                    SlotState::FaceUp => break,
                    SlotState::Empty | SlotState::Recycle => break,
                }
            }

            let mut cards = PileVec::new();
            for visible_idx in 0..13usize {
                let label = format!("pile-{}-visible-{}", pile_idx + 1, visible_idx + 1);
                let top = layout.tableau_top
                    + layout.hidden_fan_y * hidden_count as f32
                    + layout.visible_fan_y * visible_idx as f32;
                let rect = Self::card_rect(layout, image.width(), image.height(), pile_idx, top);
                let state = self.classify_tableau_visible_state(
                    &image,
                    rect,
                    layout,
                    image.height(),
                    &label,
                );
                overlay_rects.push((rect, state));
                if state != SlotState::FaceUp {
                    break;
                }
                let recognition = self.recognize_card(&image, rect, &label).ok();
                if let Some(card) = recognition.as_ref().and_then(|result| result.card) {
                    cards.push(card);
                }
                self.record_slot(
                    &mut slot_reports,
                    label,
                    rect,
                    image.width(),
                    image.height(),
                    state,
                    recognition,
                );
            }

            ObservedPile { hidden_count, cards }
        });

        let mut waste = ArrayVec::<Card, { N_DECK_CARDS as usize }>::new();
        for (idx, rect) in self
            .waste_rects(layout, image.width(), image.height())
            .into_iter()
            .enumerate()
        {
            let label = format!("waste-{}", idx + 1);
            let state = self.classify_general_state(&image, rect);
            overlay_rects.push((rect, state));
            let recognition = if state == SlotState::FaceUp {
                Some(self.recognize_card(&image, rect, &label)?)
            } else {
                None
            };
            if let Some(card) = recognition.as_ref().and_then(|result| result.card) {
                waste.push(card);
            }
            self.record_slot(
                &mut slot_reports,
                label,
                rect,
                image.width(),
                image.height(),
                state,
                recognition,
            );
        }

        let stock_rect = Rect::from_norm(
            image.width(),
            image.height(),
            layout.column_lefts[6],
            layout.top_row_top,
            layout.card_width,
            layout.card_height,
        );
        let stock_state = self.classify_stock_state(&image, stock_rect);
        overlay_rects.push((stock_rect, stock_state));
        self.record_slot(
            &mut slot_reports,
            "stock",
            stock_rect,
            image.width(),
            image.height(),
            stock_state,
            None,
        );

        let board = ObservedBoard {
            foundation,
            piles,
            waste,
            stock_count: 0,
            stock_present: matches!(stock_state, SlotState::FaceDown | SlotState::Recycle),
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

        Ok(RecognitionReport {
            annotated_path: self.save_annotated_overlay(&image, &overlay_rects, png_path)?,
            board,
            slots: slot_reports,
            image_width: image.width(),
            image_height: image.height(),
        })
    }

    pub fn inspect_png_with_calibration(
        &self,
        png_path: &Path,
        calibration: &SolitaireCashCalibration,
    ) -> Result<RecognitionReport, AdapterError> {
        let mut recognizer = self.clone().with_calibration(*calibration);
        recognizer.inspect_png(png_path, &calibration.layout)
    }

    pub fn preview_png_with_calibration(
        &self,
        png_path: &Path,
        calibration: &SolitaireCashCalibration,
        selected_slot: Option<&str>,
    ) -> Result<CalibrationPreview, AdapterError> {
        let mut recognizer = self.clone().with_calibration(*calibration);
        let report = recognizer.inspect_png(png_path, &calibration.layout)?;
        let Some(selected_label) = selected_slot else {
            return Ok(CalibrationPreview {
                report,
                selected_slot: None,
            });
        };

        let image = image::open(png_path).map_err(|err| {
            AdapterError::RecognitionError(format!(
                "failed to load screenshot {}: {err}",
                png_path.display()
            ))
        })?;
        let selected = report
            .slots
            .iter()
            .find(|slot| slot.label == selected_label && slot.state == SlotState::FaceUp)
            .cloned();
        let Some(slot) = selected else {
            return Ok(CalibrationPreview {
                report,
                selected_slot: None,
            });
        };

        let image_width = report.image_width;
        let image_height = report.image_height;
        let slot_rect = Rect::from_normalized(image_width, image_height, slot.rect);
        let face_rect = recognizer.locate_face_anchor(&image, slot_rect);
        let rank_rect = recognizer.rank_rect(face_rect);
        let suit_rect = recognizer.suit_rect(face_rect);
        let rank_gray = recognizer.grayscale_crop(&image, rank_rect);
        let suit_gray = recognizer.grayscale_crop(&image, suit_rect);
        let rank_report = recognizer.templates.match_rank(&rank_gray)?;
        let suit_report = recognizer.templates.match_suit(&suit_gray)?;

        Ok(CalibrationPreview {
            report,
            selected_slot: Some(SelectedSlotPreview {
                label: slot.label,
                face_rect: face_rect.to_normalized(image_width, image_height),
                rank_rect: rank_rect.to_normalized(image_width, image_height),
                suit_rect: suit_rect.to_normalized(image_width, image_height),
                rank_candidates: rank_report.candidates.clone(),
                suit_candidates: suit_report.candidates.clone(),
                rank_raw_png: recognizer.image_to_png_bytes(&image.crop_imm(
                    rank_rect.x,
                    rank_rect.y,
                    rank_rect.width,
                    rank_rect.height,
                ))?,
                suit_raw_png: recognizer.image_to_png_bytes(&image.crop_imm(
                    suit_rect.x,
                    suit_rect.y,
                    suit_rect.width,
                    suit_rect.height,
                ))?,
                rank_mask_png: recognizer.gray_to_png_bytes(&rank_report.normalized_mask)?,
                suit_mask_png: recognizer.gray_to_png_bytes(&suit_report.normalized_mask)?,
            }),
        })
    }
}

impl PngBoardRecognizer for PapayaSolitaireCashRecognizer {
    fn recognize_board_from_png(
        &mut self,
        png_path: &Path,
        layout: &SolitaireCashLayout,
    ) -> Result<ObservedBoard, AdapterError> {
        self.inspect_png(png_path, layout).map(|report| report.board)
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
        let absolute = self.region.point_to_screen(point);
        self.debug.log(format!(
            "tap normalized=({:.3},{:.3}) absolute=({:.1},{:.1})",
            point.x, point.y, absolute.0, absolute.1
        ));
        self.mouse.tap_abs(absolute)
    }

    fn drag(&mut self, from: Point, to: Point) -> Result<(), AdapterError> {
        let from_abs = self.region.point_to_screen(from);
        let to_abs = self.region.point_to_screen(to);
        self.debug.log(format!(
            "drag normalized=({:.3},{:.3})->({:.3},{:.3}) absolute=({:.1},{:.1})->({:.1},{:.1})",
            from.x, from.y, to.x, to.y, from_abs.0, from_abs.1, to_abs.0, to_abs.1
        ));
        self.mouse.drag_abs(from_abs, to_abs)
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
        self.debug.log(format!(
            "running screencapture for region x={} y={} width={} height={}",
            self.region.x, self.region.y, self.region.width, self.region.height
        ));
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
