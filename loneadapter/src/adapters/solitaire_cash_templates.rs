use std::path::{Path, PathBuf};

use image::imageops::FilterType;
use image::{GrayImage, ImageBuffer, Luma};
use serde::Serialize;

use crate::adapter::AdapterError;

const RANK_TEMPLATE_SIZE: (u32, u32) = (28, 36);
const SUIT_TEMPLATE_SIZE: (u32, u32) = (24, 24);
const MAX_ACCEPT_SCORE: f32 = 0.42;
const MIN_MARGIN: f32 = 0.03;

#[derive(Debug, Clone, Serialize)]
pub struct MatchCandidate {
    pub label: String,
    pub score: f32,
}

#[derive(Debug, Clone)]
pub struct MatchReport {
    pub accepted: Option<String>,
    pub candidates: Vec<MatchCandidate>,
    pub normalized_mask: GrayImage,
}

#[derive(Debug, Clone)]
struct GlyphTemplate {
    label: String,
    mask: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct TemplateLibrary {
    rank_templates: Vec<GlyphTemplate>,
    suit_templates: Vec<GlyphTemplate>,
}

impl TemplateLibrary {
    pub fn load(asset_dir: &Path) -> Result<Self, AdapterError> {
        let rank_dir = asset_dir.join("ranks");
        let suit_dir = asset_dir.join("suits");
        let rank_templates = Self::load_rank_templates(&rank_dir)?;
        let suit_templates = Self::load_suit_templates(asset_dir, &suit_dir)?;
        Ok(Self {
            rank_templates,
            suit_templates,
        })
    }

    pub fn match_rank(&self, crop: &GrayImage) -> Result<MatchReport, AdapterError> {
        self.match_templates(crop, &self.rank_templates, RANK_TEMPLATE_SIZE)
    }

    pub fn match_suit(&self, crop: &GrayImage) -> Result<MatchReport, AdapterError> {
        self.match_templates(crop, &self.suit_templates, SUIT_TEMPLATE_SIZE)
    }

    fn load_rank_templates(rank_dir: &Path) -> Result<Vec<GlyphTemplate>, AdapterError> {
        let labels = ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K"];
        let mut templates = Vec::with_capacity(labels.len());
        let mut missing = Vec::new();
        for label in labels {
            let path = rank_dir.join(format!("{label}.png"));
            if !path.exists() {
                missing.push(path);
                continue;
            }
            templates.push(Self::load_template(path, label, RANK_TEMPLATE_SIZE)?);
        }
        if !missing.is_empty() {
            let expected = missing
                .into_iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(AdapterError::RecognitionError(format!(
                "missing rank template assets: {expected}"
            )));
        }
        Ok(templates)
    }

    fn load_suit_templates(
        asset_dir: &Path,
        suit_dir: &Path,
    ) -> Result<Vec<GlyphTemplate>, AdapterError> {
        let candidates = [
            ("C", vec![suit_dir.join("C.png"), asset_dir.join("Club.png")]),
            ("D", vec![suit_dir.join("D.png"), asset_dir.join("Diamond.png")]),
            ("H", vec![suit_dir.join("H.png"), asset_dir.join("Heart.png")]),
            ("S", vec![suit_dir.join("S.png"), asset_dir.join("Spade.png")]),
        ];
        let mut templates = Vec::with_capacity(candidates.len());
        let mut missing = Vec::new();
        for (label, paths) in candidates {
            if let Some(path) = paths.into_iter().find(|path| path.exists()) {
                templates.push(Self::load_template(path, label, SUIT_TEMPLATE_SIZE)?);
            } else {
                missing.push(format!("{}/{}.png", suit_dir.display(), label));
            }
        }
        if !missing.is_empty() {
            return Err(AdapterError::RecognitionError(format!(
                "missing suit template assets: {}",
                missing.join(", ")
            )));
        }
        Ok(templates)
    }

    fn load_template(
        path: PathBuf,
        label: &str,
        size: (u32, u32),
    ) -> Result<GlyphTemplate, AdapterError> {
        let image = image::open(&path).map_err(|err| {
            AdapterError::RecognitionError(format!(
                "failed to load template {}: {err}",
                path.display()
            ))
        })?;
        let gray = image.to_luma8();
        let prepared = prepare_mask(&gray, size).ok_or_else(|| {
            AdapterError::RecognitionError(format!(
                "failed to normalize template {}",
                path.display()
            ))
        })?;
        Ok(GlyphTemplate {
            label: label.to_string(),
            mask: image_from_mask(&prepared.mask, size.0, size.1)
                .pixels()
                .map(|pixel| u8::from(pixel[0] == 0))
                .collect(),
        })
    }

    fn match_templates(
        &self,
        crop: &GrayImage,
        templates: &[GlyphTemplate],
        size: (u32, u32),
    ) -> Result<MatchReport, AdapterError> {
        let prepared = prepare_mask(crop, size).ok_or_else(|| {
            AdapterError::RecognitionError("failed to isolate glyph from crop".into())
        })?;
        let normalized_mask = image_from_mask(&prepared.mask, size.0, size.1);
        let mut candidates = templates
            .iter()
            .map(|template| MatchCandidate {
                label: template.label.clone(),
                score: hamming_score(&prepared.mask, &template.mask),
            })
            .collect::<Vec<_>>();
        candidates.sort_by(|a, b| a.score.partial_cmp(&b.score).unwrap());
        let accepted = candidates.first().and_then(|best| {
            let margin = candidates
                .get(1)
                .map_or(1.0, |runner_up| runner_up.score - best.score);
            (best.score <= MAX_ACCEPT_SCORE && margin >= MIN_MARGIN).then(|| best.label.clone())
        });
        Ok(MatchReport {
            accepted,
            candidates,
            normalized_mask,
        })
    }
}

#[derive(Debug, Clone)]
struct PreparedMask {
    mask: Vec<u8>,
}

fn prepare_mask(image: &GrayImage, size: (u32, u32)) -> Option<PreparedMask> {
    let threshold = otsu_threshold(image);
    let binary = ImageBuffer::from_fn(image.width(), image.height(), |x, y| {
        let px = image.get_pixel(x, y)[0];
        Luma([u8::from(px <= threshold)])
    });
    let bounds = largest_component_bounds(&binary)?;
    let cropped = image::imageops::crop_imm(
        &binary,
        bounds.0,
        bounds.1,
        bounds.2,
        bounds.3,
    )
    .to_image();
    let resized = image::imageops::resize(&cropped, size.0, size.1, FilterType::Nearest);
    Some(PreparedMask {
        mask: resized
            .pixels()
            .map(|pixel| u8::from(pixel[0] > 0))
            .collect(),
    })
}

fn image_from_mask(mask: &[u8], width: u32, height: u32) -> GrayImage {
    ImageBuffer::from_fn(width, height, |x, y| {
        let idx = (y * width + x) as usize;
        Luma([if mask.get(idx).copied().unwrap_or_default() == 1 {
            0
        } else {
            255
        }])
    })
}

fn hamming_score(a: &[u8], b: &[u8]) -> f32 {
    let len = a.len().min(b.len()).max(1);
    let mismatches = a
        .iter()
        .zip(b.iter())
        .map(|(lhs, rhs)| usize::from(lhs != rhs))
        .sum::<usize>();
    mismatches as f32 / len as f32
}

fn otsu_threshold(image: &GrayImage) -> u8 {
    let mut histogram = [0u32; 256];
    for pixel in image.pixels() {
        histogram[pixel[0] as usize] += 1;
    }

    let total = image.width().saturating_mul(image.height()).max(1);
    let mut sum = 0f64;
    for (idx, count) in histogram.iter().enumerate() {
        sum += idx as f64 * f64::from(*count);
    }

    let mut sum_background = 0f64;
    let mut weight_background = 0u32;
    let mut max_variance = -1f64;
    let mut threshold = 127u8;

    for (idx, count) in histogram.iter().enumerate() {
        weight_background += *count;
        if weight_background == 0 {
            continue;
        }
        let weight_foreground = total - weight_background;
        if weight_foreground == 0 {
            break;
        }
        sum_background += idx as f64 * f64::from(*count);
        let mean_background = sum_background / f64::from(weight_background);
        let mean_foreground = (sum - sum_background) / f64::from(weight_foreground);
        let variance = f64::from(weight_background)
            * f64::from(weight_foreground)
            * (mean_background - mean_foreground).powi(2);
        if variance > max_variance {
            max_variance = variance;
            threshold = idx as u8;
        }
    }
    threshold
}

fn largest_component_bounds(binary: &GrayImage) -> Option<(u32, u32, u32, u32)> {
    let width = binary.width() as i32;
    let height = binary.height() as i32;
    let mut visited = vec![false; (width * height) as usize];
    let mut best = None;

    for y in 0..height {
        for x in 0..width {
            let idx = (y * width + x) as usize;
            if visited[idx] || binary.get_pixel(x as u32, y as u32)[0] == 0 {
                continue;
            }
            let mut queue = std::collections::VecDeque::from([(x, y)]);
            visited[idx] = true;
            let mut area = 0usize;
            let (mut min_x, mut min_y, mut max_x, mut max_y) = (x, y, x, y);
            while let Some((cx, cy)) = queue.pop_front() {
                area += 1;
                min_x = min_x.min(cx);
                min_y = min_y.min(cy);
                max_x = max_x.max(cx);
                max_y = max_y.max(cy);
                for (nx, ny) in [
                    (cx + 1, cy),
                    (cx - 1, cy),
                    (cx, cy + 1),
                    (cx, cy - 1),
                ] {
                    if nx < 0 || ny < 0 || nx >= width || ny >= height {
                        continue;
                    }
                    let nidx = (ny * width + nx) as usize;
                    if visited[nidx] || binary.get_pixel(nx as u32, ny as u32)[0] == 0 {
                        continue;
                    }
                    visited[nidx] = true;
                    queue.push_back((nx, ny));
                }
            }
            let candidate = (
                area,
                min_x as u32,
                min_y as u32,
                (max_x - min_x + 1) as u32,
                (max_y - min_y + 1) as u32,
            );
            if best.as_ref().is_none_or(|current: &(usize, u32, u32, u32, u32)| candidate.0 > current.0) {
                best = Some(candidate);
            }
        }
    }

    best.map(|(_, x, y, width, height)| (x, y, width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use image::{ImageBuffer, Luma};

    fn make_mask_image(width: u32, height: u32, points: &[(u32, u32)]) -> GrayImage {
        ImageBuffer::from_fn(width, height, |x, y| {
            if points.contains(&(x, y)) {
                Luma([0])
            } else {
                Luma([255])
            }
        })
    }

    #[test]
    fn prepare_mask_isolates_largest_component() {
        let mut image = ImageBuffer::from_pixel(12, 12, Luma([255]));
        for (x, y) in [(1, 1), (1, 2), (2, 1), (2, 2), (8, 8)] {
            image.put_pixel(x, y, Luma([0]));
        }
        let prepared = prepare_mask(&image, (8, 8)).expect("mask");
        assert_eq!(prepared.mask.len(), 64);
        assert!(prepared.mask.iter().any(|value| *value == 1));
    }

    #[test]
    fn template_matching_prefers_lowest_hamming_score() {
        let dir = std::env::temp_dir().join(format!(
            "solitaire-cash-template-test-{}",
            std::process::id()
        ));
        let rank_dir = dir.join("ranks");
        let suit_dir = dir.join("suits");
        fs::create_dir_all(&rank_dir).unwrap();
        fs::create_dir_all(&suit_dir).unwrap();

        let labels = ["A", "2", "3", "4", "5", "6", "7", "8", "9", "10", "J", "Q", "K"];
        for label in labels {
            let image = if label == "A" {
                make_mask_image(12, 12, &[(1, 1), (1, 2), (2, 1), (2, 2)])
            } else {
                make_mask_image(12, 12, &[(8, 8), (8, 9), (9, 8), (9, 9)])
            };
            image.save(rank_dir.join(format!("{label}.png"))).unwrap();
        }
        for (label, points) in [
            ("C", vec![(1, 2), (1, 3), (1, 4), (2, 2), (2, 4)]),
            ("D", vec![(8, 7), (8, 8), (8, 9), (9, 8), (10, 8)]),
            ("H", vec![(2, 8), (2, 9), (3, 8), (4, 8), (4, 9)]),
            ("S", vec![(8, 1), (9, 1), (9, 2), (8, 3), (9, 3)]),
        ] {
            make_mask_image(12, 12, &points)
                .save(suit_dir.join(format!("{label}.png")))
                .unwrap();
        }

        let library = TemplateLibrary::load(&dir).unwrap();
        let rank_report = library
            .match_rank(&make_mask_image(12, 12, &[(1, 1), (1, 2), (2, 1), (2, 2)]))
            .unwrap();
        assert_eq!(
            rank_report.candidates.first().map(|candidate| candidate.label.as_str()),
            Some("A")
        );

        let suit_report = library
            .match_suit(&make_mask_image(12, 12, &[(8, 1), (9, 1), (9, 2), (8, 3), (9, 3)]))
            .unwrap();
        assert_eq!(
            suit_report.candidates.first().map(|candidate| candidate.label.as_str()),
            Some("S")
        );

        let _ = fs::remove_dir_all(dir);
    }
}
