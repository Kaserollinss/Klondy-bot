use std::thread;
use std::time::Duration;

use arrayvec::ArrayVec;
use lonelybot::card::{Card, N_SUITS};
use lonelybot::deck::{N_DECK_CARDS, N_PILES};
use lonelybot::partial::PartialBoard;
use lonelybot::standard::{PileVec, Pos, StandardMove};
use serde::{Deserialize, Serialize};

use crate::adapter::{AdapterError, ScreenAdapter};

const DRAW_STEP: u8 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    pub fn to_rust_literal(&self) -> String {
        format!("Point {{ x: {:.6}, y: {:.6} }}", self.x, self.y)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormalizedRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl NormalizedRect {
    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub fn to_rust_literal(&self) -> String {
        format!(
            "NormalizedRect {{ x: {:.6}, y: {:.6}, width: {:.6}, height: {:.6} }}",
            self.x, self.y, self.width, self.height
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedPile {
    pub hidden_count: u8,
    pub cards: PileVec,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObservedBoard {
    pub foundation: [u8; N_SUITS as usize],
    pub piles: [ObservedPile; N_PILES as usize],
    pub waste: ArrayVec<Card, { N_DECK_CARDS as usize }>,
    pub stock_count: u8,
    pub stock_present: bool,
}

impl ObservedBoard {
    fn total_deck_cards(&self) -> u8 {
        let foundation_count: u8 = self.foundation.iter().sum();
        let pile_visible: u8 = self.piles.iter().map(|pile| pile.cards.len() as u8).sum();
        let hidden_total: u8 = self.piles.iter().map(|pile| pile.hidden_count).sum();
        52 - foundation_count - pile_visible - hidden_total
    }

    fn is_clearly_unrecognized(&self) -> bool {
        self.foundation.iter().all(|count| *count == 0)
            && self.waste.is_empty()
            && !self.stock_present
            && self
                .piles
                .iter()
                .all(|pile| pile.hidden_count == 0 && pile.cards.is_empty())
    }
}

pub trait SolitaireCashBackend {
    fn observe(&mut self, layout: &SolitaireCashLayout) -> Result<ObservedBoard, AdapterError>;

    fn tap(&mut self, _point: Point) -> Result<(), AdapterError> {
        Err(AdapterError::ExecutionError(
            "backend does not support taps".into(),
        ))
    }

    fn drag(&mut self, _from: Point, _to: Point) -> Result<(), AdapterError> {
        Err(AdapterError::ExecutionError(
            "backend does not support drags".into(),
        ))
    }

    fn can_interact(&self) -> bool {
        false
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SolitaireCashLayout {
    pub column_lefts: [f32; N_PILES as usize],
    pub card_width: f32,
    pub card_height: f32,
    pub top_row_top: f32,
    pub tableau_top: f32,
    pub hidden_fan_y: f32,
    pub visible_fan_y: f32,
    pub waste_origin: Point,
    pub stock_tap_point: Point,
    pub submit_point: Point,
    pub undo_point: Point,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SolitaireCashVisionCalibration {
    pub rank_rect: NormalizedRect,
    pub suit_rect: NormalizedRect,
    pub waste_overlap: f32,
    pub face_anchor_bright_ratio: f32,
    pub face_anchor_padding_px: u32,
    pub face_up_white_ratio: f32,
    pub face_down_purple_ratio: f32,
    pub recycle_foreground_ratio: f32,
    pub center_foreground_inset_x: f32,
    pub center_foreground_inset_y: f32,
    pub white_min_rgb: u8,
    pub purple_blue_min: u8,
    pub purple_red_min: u8,
    pub purple_blue_over_green: i32,
    pub purple_red_over_green: i32,
    pub background_rgb: [u8; 3],
    pub background_distance_threshold: i32,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SolitaireCashCalibration {
    pub layout: SolitaireCashLayout,
    pub vision: SolitaireCashVisionCalibration,
}

impl SolitaireCashLayout {
    pub fn to_rust_literal(&self) -> String {
        format!(
            "SolitaireCashLayout {{\n            column_lefts: [{}, {}, {}, {}, {}, {}, {}],\n            card_width: {:.6},\n            card_height: {:.6},\n            top_row_top: {:.6},\n            tableau_top: {:.6},\n            hidden_fan_y: {:.6},\n            visible_fan_y: {:.6},\n            waste_origin: {},\n            stock_tap_point: {},\n            submit_point: {},\n            undo_point: {},\n        }}",
            self.column_lefts[0],
            self.column_lefts[1],
            self.column_lefts[2],
            self.column_lefts[3],
            self.column_lefts[4],
            self.column_lefts[5],
            self.column_lefts[6],
            self.card_width,
            self.card_height,
            self.top_row_top,
            self.tableau_top,
            self.hidden_fan_y,
            self.visible_fan_y,
            self.waste_origin.to_rust_literal(),
            self.stock_tap_point.to_rust_literal(),
            self.submit_point.to_rust_literal(),
            self.undo_point.to_rust_literal(),
        )
    }
}

impl SolitaireCashVisionCalibration {
    pub fn to_rust_literal(&self) -> String {
        format!(
            "SolitaireCashVisionCalibration {{\n            rank_rect: {},\n            suit_rect: {},\n            waste_overlap: {:.6},\n            face_anchor_bright_ratio: {:.6},\n            face_anchor_padding_px: {},\n            face_up_white_ratio: {:.6},\n            face_down_purple_ratio: {:.6},\n            recycle_foreground_ratio: {:.6},\n            center_foreground_inset_x: {:.6},\n            center_foreground_inset_y: {:.6},\n            white_min_rgb: {},\n            purple_blue_min: {},\n            purple_red_min: {},\n            purple_blue_over_green: {},\n            purple_red_over_green: {},\n            background_rgb: [{}, {}, {}],\n            background_distance_threshold: {},\n        }}",
            self.rank_rect.to_rust_literal(),
            self.suit_rect.to_rust_literal(),
            self.waste_overlap,
            self.face_anchor_bright_ratio,
            self.face_anchor_padding_px,
            self.face_up_white_ratio,
            self.face_down_purple_ratio,
            self.recycle_foreground_ratio,
            self.center_foreground_inset_x,
            self.center_foreground_inset_y,
            self.white_min_rgb,
            self.purple_blue_min,
            self.purple_red_min,
            self.purple_blue_over_green,
            self.purple_red_over_green,
            self.background_rgb[0],
            self.background_rgb[1],
            self.background_rgb[2],
            self.background_distance_threshold,
        )
    }
}

impl SolitaireCashCalibration {
    pub fn to_rust_literal(&self) -> String {
        format!(
            "SolitaireCashCalibration {{\n        layout: {},\n        vision: {},\n    }}",
            self.layout.to_rust_literal(),
            self.vision.to_rust_literal(),
        )
    }
}

impl Default for SolitaireCashCalibration {
    fn default() -> Self {
        Self {
            layout: SolitaireCashLayout {
                // Normalized from the mirrored macOS capture geometry (1280 x 1960).
                column_lefts: [
                    0.096875,
                    0.2125,
                    0.328125,
                    0.44375,
                    0.559375,
                    0.675,
                    0.790625,
                ],
                card_width: 0.112134,
                card_height: 0.110296,
                top_row_top: 0.233735,
                tableau_top: 0.420609,
                hidden_fan_y: 0.015430,
                visible_fan_y: 0.029592,
                waste_origin: Point::new(0.582122, 0.235524),
                stock_tap_point: Point::new(0.837657, 0.287454),
                submit_point: Point::new(0.203347, 0.948085),
                undo_point: Point::new(0.800000, 0.950942),
            },
            vision: SolitaireCashVisionCalibration {
                rank_rect: NormalizedRect::new(0.044975, 0.021880, 0.326308, 0.235974),
                suit_rect: NormalizedRect::new(0.593622, 0.013222, 0.379428, 0.230731),
                waste_overlap: 0.358209,
                face_anchor_bright_ratio: 0.15,
                face_anchor_padding_px: 2,
                face_up_white_ratio: 0.40,
                face_down_purple_ratio: 0.08,
                recycle_foreground_ratio: 0.03,
                center_foreground_inset_x: 0.25,
                center_foreground_inset_y: 0.25,
                white_min_rgb: 220,
                purple_blue_min: 90,
                purple_red_min: 70,
                purple_blue_over_green: 20,
                purple_red_over_green: 10,
                background_rgb: [46, 104, 62],
                background_distance_threshold: 45,
            },
        }
    }
}

impl Default for SolitaireCashLayout {
    fn default() -> Self {
        SolitaireCashCalibration::default().layout
    }
}

impl SolitaireCashLayout {
    fn card_center(&self, column: usize, top: f32) -> Point {
        Point::new(
            self.column_lefts[column] + self.card_width / 2.0,
            top + self.card_height / 2.0,
        )
    }

    fn foundation_point(&self, suit: u8) -> Point {
        self.card_center(suit as usize, self.top_row_top)
    }

    pub(crate) fn stock_point(&self) -> Point {
        self.stock_tap_point
    }

    pub(crate) fn waste_point(&self) -> Point {
        Point::new(
            self.waste_origin.x + self.card_width / 2.0,
            self.waste_origin.y + self.card_height / 2.0,
        )
    }

    pub(crate) fn undo_point(&self) -> Point {
        self.undo_point
    }

    pub fn submit_point(&self) -> Point {
        self.submit_point
    }

    fn tableau_card_point(&self, pile: usize, hidden_count: u8, visible_index: usize) -> Point {
        let top = self.tableau_top
            + self.hidden_fan_y * hidden_count as f32
            + self.visible_fan_y * visible_index as f32;
        Point::new(
            self.column_lefts[pile] + self.card_width / 2.0,
            top + self.card_height * 0.32,
        )
    }

    fn tableau_drop_point(&self, pile: usize, observed: &ObservedPile) -> Point {
        if observed.cards.is_empty() {
            self.card_center(pile, self.tableau_top)
        } else {
            self.tableau_card_point(pile, observed.hidden_count, observed.cards.len() - 1)
        }
    }
}

#[derive(Debug, Clone)]
pub struct SolitaireCashAdapter<B> {
    backend: B,
    layout: SolitaireCashLayout,
    draw_step: u8,
    scan_full_deck: bool,
    settle_time: Duration,
    scan_tap_delay: Duration,
    max_deck_scan_taps: u8,
    debug: bool,
    last_observation: Option<ObservedBoard>,
}

impl<B> SolitaireCashAdapter<B> {
    pub fn new(backend: B) -> Self {
        Self {
            backend,
            layout: SolitaireCashLayout::default(),
            draw_step: 3,
            scan_full_deck: true,
            settle_time: Duration::from_millis(500),
            scan_tap_delay: Duration::from_millis(180),
            max_deck_scan_taps: 64,
            debug: false,
            last_observation: None,
        }
    }

    pub fn with_layout(mut self, layout: SolitaireCashLayout) -> Self {
        self.layout = layout;
        self
    }

    pub fn with_settle_time(mut self, settle_time: Duration) -> Self {
        self.settle_time = settle_time;
        self
    }

    pub fn with_scan_tap_delay(mut self, scan_tap_delay: Duration) -> Self {
        self.scan_tap_delay = scan_tap_delay;
        self
    }

    pub fn with_full_deck_scan(mut self, enabled: bool) -> Self {
        self.scan_full_deck = enabled;
        self
    }

    pub fn with_debug(mut self, debug: bool) -> Self {
        self.debug = debug;
        self
    }

    pub fn backend(&self) -> &B {
        &self.backend
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
}

impl<B: SolitaireCashBackend> SolitaireCashAdapter<B> {
    fn debug_log(&self, message: impl AsRef<str>) {
        if self.debug {
            eprintln!("[solitaire-cash] {}", message.as_ref());
        }
    }

    fn describe_point(point: Point) -> String {
        format!("({:.3},{:.3})", point.x, point.y)
    }

    fn describe_board(&self, board: &PartialBoard) -> String {
        let pile_summary = board
            .pile_cards
            .iter()
            .enumerate()
            .map(|(idx, pile)| {
                format!(
                    "p{} hidden={} visible={}",
                    idx + 1,
                    board.hidden_counts[idx],
                    pile.len()
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "foundation={:?} waste_visible={} stock_count={} known_deck={} {pile_summary}",
            board.foundation,
            board.waste.len(),
            board.stock_count,
            board.known_deck_order.as_ref().map_or(0, |deck| deck.len()),
        )
    }

    fn taps_to_cycle_start(total_deck_cards: u8, draw_cur: u8) -> u8 {
        if total_deck_cards == 0 {
            0
        } else if draw_cur == 0 {
            0
        } else {
            (total_deck_cards - draw_cur).div_ceil(DRAW_STEP) + 1
        }
    }

    fn visible_waste_len_for_draw_cur(draw_cur: u8) -> u8 {
        draw_cur.min(DRAW_STEP)
    }

    fn infer_stock_count(
        &self,
        total_deck_cards: u8,
        observed_waste: &[Card],
        taps_to_cycle_start: u8,
        known_deck_order: Option<&ArrayVec<Card, { N_DECK_CARDS as usize }>>,
    ) -> Result<u8, AdapterError> {
        for draw_cur in 0..=total_deck_cards {
            let expected_taps = Self::taps_to_cycle_start(total_deck_cards, draw_cur);
            let expected_visible = Self::visible_waste_len_for_draw_cur(draw_cur) as usize;
            let waste_matches = known_deck_order.is_none_or(|deck_order| {
                let draw_cur = draw_cur as usize;
                let start = draw_cur.saturating_sub(expected_visible);
                deck_order
                    .get(start..draw_cur)
                    .is_some_and(|cards| cards == observed_waste)
            });
            if expected_taps == taps_to_cycle_start
                && expected_visible == observed_waste.len()
                && waste_matches
            {
                return Ok(total_deck_cards - draw_cur);
            }
        }

        Err(AdapterError::RecognitionError(format!(
            "could not infer stock count from total_deck_cards={total_deck_cards}, observed_waste_len={}, taps_to_cycle_start={taps_to_cycle_start}",
            observed_waste.len()
        )))
    }

    fn tap_and_wait(&mut self, point: Point, delay: Duration) -> Result<(), AdapterError> {
        self.backend.tap(point)?;
        thread::sleep(delay);
        Ok(())
    }

    fn observe(&mut self) -> Result<ObservedBoard, AdapterError> {
        self.backend.observe(&self.layout)
    }

    fn scan_known_deck_order(
        &mut self,
        original: &ObservedBoard,
    ) -> Result<(Option<ArrayVec<Card, { N_DECK_CARDS as usize }>>, u8), AdapterError> {
        if !self.scan_full_deck || !self.backend.can_interact() {
            self.debug_log(format!(
                "full deck scan skipped: enabled={} can_interact={}",
                self.scan_full_deck,
                self.backend.can_interact()
            ));
            return Ok((None, 0));
        }

        let total_deck_cards = original.total_deck_cards();
        if total_deck_cards == 0 {
            return Ok((Some(ArrayVec::new()), 0));
        }

        let mut working = original.clone();
        let mut taps_used = 0u8;
        let stock_point = self.layout.stock_point();
        let undo_point = self.layout.undo_point();
        self.debug_log(format!(
            "starting deck scan: waste_visible={} stock_present={} total_deck_cards={} stock_point={} undo_point={}",
            working.waste.len(),
            working.stock_present,
            total_deck_cards,
            Self::describe_point(stock_point),
            Self::describe_point(undo_point)
        ));

        while !(working.stock_present && working.waste.is_empty()) {
            if taps_used >= self.max_deck_scan_taps {
                return Err(AdapterError::RecognitionError(
                    "deck scan exceeded tap budget before reset".into(),
                ));
            }
            self.tap_and_wait(stock_point, self.scan_tap_delay)?;
            taps_used += 1;
            working = self.observe()?;
            self.debug_log(format!(
                "deck scan reset tap {}: waste_visible={} stock_present={}",
                taps_used,
                working.waste.len(),
                working.stock_present
            ));
        }

        let taps_to_cycle_start = taps_used;
        let mut known_slots = vec![None; total_deck_cards as usize];
        let mut draw_cur = 0u8;
        let mut completed_cycles = 0u8;
        let mut filled_at_cycle_start = 0usize;
        while known_slots.iter().any(Option::is_none) {
            if taps_used >= self.max_deck_scan_taps {
                return Err(AdapterError::RecognitionError(
                    "deck scan exceeded tap budget during stock traversal".into(),
                ));
            }
            self.tap_and_wait(stock_point, self.scan_tap_delay)?;
            taps_used += 1;
            working = self.observe()?;

            if working.stock_present && working.waste.is_empty() {
                completed_cycles += 1;
                let filled_now = known_slots.iter().filter(|slot| slot.is_some()).count();
                self.debug_log(format!(
                    "deck scan reset/recycle tap {}: completed_cycles={} filled={}/{}",
                    taps_used, completed_cycles, filled_now, total_deck_cards
                ));
                if filled_now == total_deck_cards as usize {
                    break;
                }
                if filled_now == filled_at_cycle_start {
                    return Err(AdapterError::RecognitionError(format!(
                        "deck scan stalled after {} full cycle(s): captured {} cards, expected {}",
                        completed_cycles, filled_now, total_deck_cards
                    )));
                }
                filled_at_cycle_start = filled_now;
                draw_cur = 0;
                continue;
            }

            let next_draw_cur = (draw_cur + self.draw_step).min(total_deck_cards);
            let visible_len = working.waste.len().min(next_draw_cur as usize);
            let slot_start = next_draw_cur as usize - visible_len;
            let slot_end = next_draw_cur as usize;

            for (slot, card) in known_slots[slot_start..slot_end]
                .iter_mut()
                .zip(working.waste.iter())
            {
                match slot {
                    Some(existing) if *existing != *card => {
                        return Err(AdapterError::RecognitionError(format!(
                            "deck scan observed conflicting cards for slot {}: saw {} then {}",
                            slot_start, existing, card
                        )));
                    }
                    Some(_) => {}
                    None => *slot = Some(*card),
                }
            }
            draw_cur = next_draw_cur;
            let filled_now = known_slots.iter().filter(|slot| slot.is_some()).count();
            self.debug_log(format!(
                "deck scan reveal tap {}: draw_cur={} filled={}/{} waste_visible={} stock_present={}",
                taps_used,
                draw_cur,
                filled_now,
                total_deck_cards,
                working.waste.len(),
                working.stock_present
            ));
        }

        let mut known_deck_order = ArrayVec::<Card, { N_DECK_CARDS as usize }>::new();
        for (idx, slot) in known_slots.into_iter().enumerate() {
            let card = slot.ok_or_else(|| {
                AdapterError::RecognitionError(format!(
                    "deck scan missed card at deck position {}",
                    idx
                ))
            })?;
            known_deck_order
                .try_push(card)
                .map_err(|_| {
                    AdapterError::RecognitionError(
                        "detected more deck cards than fit in a Klondike stock".into(),
                    )
                })?;
        }

        for _ in 0..taps_used {
            self.tap_and_wait(undo_point, self.scan_tap_delay)?;
        }
        self.debug_log(format!("deck scan restored using {} undo taps", taps_used));

        let restored = self.observe()?;
        if restored.foundation != original.foundation
            || restored.piles != original.piles
            || restored.stock_present != original.stock_present
            || restored.waste.len() != original.waste.len()
        {
            return Err(AdapterError::RecognitionError(
                "failed to restore board after deck scan".into(),
            ));
        }

        Ok((Some(known_deck_order), taps_to_cycle_start))
    }

    fn source_point_for_move(
        &self,
        observed: &ObservedBoard,
        m: &StandardMove,
    ) -> Result<Point, AdapterError> {
        match m.from {
            Pos::Deck => Ok(self.layout.waste_point()),
            Pos::Pile(from) => {
                let pile = &observed.piles[from as usize];
                let card_index = pile
                    .cards
                    .iter()
                    .position(|card| *card == m.card)
                    .ok_or_else(|| {
                        AdapterError::ExecutionError(format!(
                            "card {m} not found in source pile {}",
                            from + 1
                        ))
                    })?;
                Ok(self
                    .layout
                    .tableau_card_point(from as usize, pile.hidden_count, card_index))
            }
            Pos::Stack(suit) => Ok(self.layout.foundation_point(suit)),
        }
    }

    fn destination_point_for_move(
        &self,
        observed: &ObservedBoard,
        m: &StandardMove,
    ) -> Result<Point, AdapterError> {
        match m.to {
            Pos::Deck => Ok(self.layout.stock_point()),
            Pos::Pile(to) => Ok(self
                .layout
                .tableau_drop_point(to as usize, &observed.piles[to as usize])),
            Pos::Stack(suit) => Ok(self.layout.foundation_point(suit)),
        }
    }
}

impl<B: SolitaireCashBackend> ScreenAdapter for SolitaireCashAdapter<B> {
    fn read_board(&mut self) -> Result<PartialBoard, AdapterError> {
        self.debug_log("startup step 1/4: capturing board");
        let observed = self.observe()?;
        self.debug_log(format!(
            "startup step 2/4: recognized visible state waste_visible={} stock_present={} total_deck_cards={}",
            observed.waste.len(),
            observed.stock_present,
            observed.total_deck_cards()
        ));
        if observed.is_clearly_unrecognized() {
            self.debug_log(
                "recognizer returned an impossible empty board; refusing to interact with the screen",
            );
            return Err(AdapterError::RecognitionError(
                "recognized an impossible empty board; the capture region is correct, but the Solitaire Cash layout calibration does not match this mirrored window yet".into(),
            ));
        }
        if self.scan_full_deck && self.backend.can_interact() && observed.total_deck_cards() > 0 {
            self.debug_log(format!(
                "startup step 3/4: full deck scan enabled, will use stock_point={} and undo_point={}",
                Self::describe_point(self.layout.stock_point()),
                Self::describe_point(self.layout.undo_point())
            ));
        }
        let (known_deck_order, taps_to_cycle_start) = self.scan_known_deck_order(&observed)?;
        let total_deck_cards = observed.total_deck_cards();
        let stock_count = if let Some(_) = known_deck_order {
            self.infer_stock_count(
                total_deck_cards,
                observed.waste.as_slice(),
                taps_to_cycle_start,
                known_deck_order.as_ref(),
            )?
        } else {
            observed.stock_count
        };
        let waste = if let Some(deck_order) = &known_deck_order {
            let draw_cur = total_deck_cards.saturating_sub(stock_count) as usize;
            let visible_len = observed.waste.len();
            let start = draw_cur.saturating_sub(visible_len);
            deck_order[start..draw_cur].iter().copied().collect()
        } else {
            observed.waste.clone()
        };
        self.debug_log(format!(
            "startup step 4/4: solved visible stock_count={} waste_visible={} known_deck={}",
            stock_count,
            waste.len(),
            known_deck_order.as_ref().map_or(0, |deck| deck.len())
        ));

        let board = PartialBoard {
            pile_cards: core::array::from_fn(|idx| observed.piles[idx].cards.clone()),
            hidden_counts: core::array::from_fn(|idx| observed.piles[idx].hidden_count),
            foundation: observed.foundation,
            waste,
            known_deck_order,
            stock_count,
            draw_step: self.draw_step,
        };

        board.validate().map_err(|err| {
            AdapterError::RecognitionError(format!("recognized invalid Solitaire Cash board: {err:?}"))
        })?;

        self.debug_log(self.describe_board(&board));
        self.last_observation = Some(observed);
        Ok(board)
    }

    fn can_execute(&self) -> bool {
        self.backend.can_interact()
    }

    fn execute_move(&mut self, m: &StandardMove) -> Result<(), AdapterError> {
        if !self.backend.can_interact() {
            return Err(AdapterError::ExecutionError(
                "backend does not support move execution".into(),
            ));
        }

        if *m == StandardMove::DRAW_NEXT {
            self.debug_log("executing draw");
            return self.backend.tap(self.layout.stock_point());
        }

        let observed = self
            .last_observation
            .as_ref()
            .ok_or_else(|| AdapterError::ExecutionError("no cached board for move execution".into()))?;

        let from = self.source_point_for_move(observed, m)?;
        let to = self.destination_point_for_move(observed, m)?;
        self.debug_log(format!(
            "executing move {m} from ({:.3},{:.3}) to ({:.3},{:.3})",
            from.x, from.y, to.x, to.y
        ));
        self.backend.drag(from, to)
    }

    fn name(&self) -> &str {
        "solitaire-cash"
    }

    fn settle_time(&self) -> Duration {
        self.settle_time
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lonelybot::partial::parse_card;

    #[derive(Debug, Clone)]
    struct MockBackend {
        foundation: [u8; N_SUITS as usize],
        piles: [ObservedPile; N_PILES as usize],
        deck_order: ArrayVec<Card, { N_DECK_CARDS as usize }>,
        draw_cur: u8,
        history: Vec<u8>,
        taps: Vec<Point>,
        drags: Vec<(Point, Point)>,
        interactive: bool,
    }

    impl MockBackend {
        fn new(deck: &[&str], draw_cur: u8, interactive: bool) -> Self {
            let mut deck_order = ArrayVec::<Card, { N_DECK_CARDS as usize }>::new();
            for card in deck {
                deck_order.push(parse_card(card).unwrap());
            }
            Self {
                foundation: [0; 4],
                piles: core::array::from_fn(|_| ObservedPile {
                    hidden_count: 0,
                    cards: PileVec::new(),
                }),
                deck_order,
                draw_cur,
                history: Vec::new(),
                taps: Vec::new(),
                drags: Vec::new(),
                interactive,
            }
        }

        fn stock_tap_count(&self, layout: &SolitaireCashLayout) -> usize {
            self.taps
                .iter()
                .filter(|tap| **tap == layout.stock_point())
                .count()
        }

        fn undo_tap_count(&self, layout: &SolitaireCashLayout) -> usize {
            self.taps
                .iter()
                .filter(|tap| **tap == layout.undo_point())
                .count()
        }
    }

    impl SolitaireCashBackend for MockBackend {
        fn observe(&mut self, _layout: &SolitaireCashLayout) -> Result<ObservedBoard, AdapterError> {
            let start = self.draw_cur.saturating_sub(3) as usize;
            let end = self.draw_cur as usize;
            let waste = self.deck_order[start..end].iter().copied().collect();

            Ok(ObservedBoard {
                foundation: self.foundation,
                piles: self.piles.clone(),
                waste,
                stock_count: self.deck_order.len() as u8 - self.draw_cur,
                stock_present: self.draw_cur < self.deck_order.len() as u8,
            })
        }

        fn tap(&mut self, point: Point) -> Result<(), AdapterError> {
            self.taps.push(point);
            if !self.interactive {
                return Err(AdapterError::ExecutionError("mock backend is read-only".into()));
            }

            let layout = SolitaireCashLayout::default();
            if point == layout.stock_point() {
                self.history.push(self.draw_cur);
                let len = self.deck_order.len() as u8;
                self.draw_cur = if self.draw_cur >= len {
                    0
                } else {
                    (self.draw_cur + 3).min(len)
                };
                Ok(())
            } else if point == layout.undo_point() {
                self.draw_cur = self
                    .history
                    .pop()
                    .ok_or_else(|| AdapterError::ExecutionError("nothing to undo".into()))?;
                Ok(())
            } else {
                Ok(())
            }
        }

        fn drag(&mut self, from: Point, to: Point) -> Result<(), AdapterError> {
            self.drags.push((from, to));
            if self.interactive {
                Ok(())
            } else {
                Err(AdapterError::ExecutionError("mock backend is read-only".into()))
            }
        }

        fn can_interact(&self) -> bool {
            self.interactive
        }
    }

    #[test]
    fn read_board_scans_full_draw_three_deck_and_restores_state() {
        let mut backend =
            MockBackend::new(&["QH", "QD", "QC", "QS", "KH", "KD", "KC", "KS"], 5, true);
        backend.foundation = [11; 4];
        let mut adapter = SolitaireCashAdapter::new(backend)
            .with_settle_time(Duration::ZERO)
            .with_scan_tap_delay(Duration::ZERO);

        let board = adapter.read_board().unwrap();

        let known = board.known_deck_order.expect("expected full deck order");
        let expected: ArrayVec<Card, { N_DECK_CARDS as usize }> = [
            "QH", "QD", "QC", "QS", "KH", "KD", "KC", "KS",
        ]
        .into_iter()
        .map(|card| parse_card(card).unwrap())
        .collect();

        assert_eq!(board.stock_count, 3);
        assert_eq!(board.waste.len(), 3);
        assert_eq!(known, expected);
        assert_eq!(adapter.backend().draw_cur, 5);
        assert_eq!(adapter.backend().stock_tap_count(&adapter.layout), 5);
        assert_eq!(adapter.backend().undo_tap_count(&adapter.layout), 5);
    }

    #[test]
    fn read_board_without_interaction_returns_partial_view_only() {
        let mut backend = MockBackend::new(&["KH", "KD", "KC", "QS", "KS"], 3, false);
        backend.foundation = [12, 12, 12, 11];
        let mut adapter = SolitaireCashAdapter::new(backend)
            .with_full_deck_scan(true)
            .with_settle_time(Duration::ZERO)
            .with_scan_tap_delay(Duration::ZERO);

        let board = adapter.read_board().unwrap();

        assert!(board.known_deck_order.is_none());
        assert_eq!(board.stock_count, 2);
        assert_eq!(board.waste.len(), 3);
    }

    #[test]
    fn read_board_rejects_impossible_empty_observation_before_scanning() {
        let mut backend = MockBackend::new(&[], 0, true);
        backend.foundation = [0; 4];
        let mut adapter = SolitaireCashAdapter::new(backend)
            .with_settle_time(Duration::ZERO)
            .with_scan_tap_delay(Duration::ZERO);

        match adapter.read_board() {
            Ok(_) => panic!("expected impossible empty observation to fail"),
            Err(AdapterError::RecognitionError(message)) => {
                assert!(message.contains("impossible empty board"));
            }
            Err(other) => panic!("unexpected error: {other:?}"),
        }

        assert_eq!(adapter.backend().taps.len(), 0);
    }
}
