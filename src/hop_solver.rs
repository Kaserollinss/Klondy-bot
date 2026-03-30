use core::ops::{Add, AddAssign};

use rand::RngCore;

use crate::{
    moves::Move,
    pruning::{FullPruner, Pruner},
    solver::SearchResult,
    state::{Encode, Solitaire},
    tracking::TerminateSignal,
    traverse::{traverse, Callback, Control, TpTable},
};

struct HOPSolverCallback<'a, T: TerminateSignal> {
    sign: &'a T,
    result: SearchResult,
    limit: usize,
    n_visit: usize,
    max_foundation: u8,
}

impl<T: TerminateSignal> Callback for HOPSolverCallback<'_, T> {
    type Pruner = FullPruner;

    fn on_win(&mut self, _: &Solitaire) -> Control {
        self.result = SearchResult::Solved;
        self.max_foundation = 52;
        Control::Halt
    }

    fn on_visit(&mut self, g: &Solitaire, _: Encode) -> Control {
        let f = g.get_stack().len();
        if f > self.max_foundation {
            self.max_foundation = f;
        }

        if g.is_sure_win() {
            self.result = SearchResult::Solved;
            self.max_foundation = 52;
            return Control::Halt;
        }

        if self.sign.is_terminated() {
            self.result = SearchResult::Terminated;
            return Control::Halt;
        }

        self.n_visit += 1;
        if self.n_visit > self.limit {
            self.result = SearchResult::Terminated;
            Control::Halt
        } else {
            Control::Ok
        }
    }
}

#[derive(Default, Clone, Copy)]
pub struct HopResult {
    pub total_score: usize,
    pub skips: usize,
    pub played: usize,
}

const SURE_WIN: HopResult = HopResult {
    total_score: !0,
    skips: 0,
    played: !0,
};

const SURE_LOSE: HopResult = HopResult {
    total_score: 0,
    skips: !0,
    played: !0,
};

const SKIPPED: HopResult = HopResult {
    total_score: 0,
    skips: 1,
    played: 1,
};

impl Add for HopResult {
    type Output = Self;

    fn add(self, rhs: Self) -> Self {
        Self {
            total_score: self.total_score + rhs.total_score,
            skips: self.skips + rhs.skips,
            played: self.played + rhs.played,
        }
    }
}

impl AddAssign for HopResult {
    fn add_assign(&mut self, rhs: Self) {
        self.total_score += rhs.total_score;
        self.skips += rhs.skips;
        self.played += rhs.played;
    }
}

pub fn hop_solve_game<R: RngCore, T: TerminateSignal>(
    g: &Solitaire,
    m: Move,
    rng: &mut R,
    n_times: usize,
    limit: usize,
    sign: &T,
    prune_info: &FullPruner,
) -> HopResult {
    let mut total_score = 0;
    let mut total_skips = 0;
    let mut total_played = 0;

    let mut tp = TpTable::default();

    // check if determinize
    let total_hidden: u8 = g.get_hidden().total_down_cards();
    if total_hidden <= 1 {
        // totally determinized
        let res = crate::solver::solve(&mut g.clone()).0;
        return if res == SearchResult::Solved {
            SURE_WIN
        } else if res == SearchResult::Unsolvable {
            SURE_LOSE
        } else {
            SKIPPED
        };
    }

    for _ in 0..n_times {
        let mut gg = g.clone();
        gg.hidden_shuffle(rng);
        let (rev_m, (_, extra)) = gg.do_move(m);
        let new_prune_info = FullPruner::update(prune_info, m, rev_m, extra);

        let mut callback = HOPSolverCallback {
            sign,
            result: SearchResult::Unsolvable,
            limit,
            n_visit: 0,
            max_foundation: 0,
        };
        tp.clear();
        traverse(&mut gg, new_prune_info, &mut tp, &mut callback);
        if sign.is_terminated() {
            break;
        }
        total_played += 1;
        let result = callback.result;
        match result {
            SearchResult::Solved => {
                total_score += 52;
            }
            SearchResult::Terminated => {
                total_skips += 1;
                total_score += callback.max_foundation as usize;
            }
            _ => {
                total_score += callback.max_foundation as usize;
            }
        }
    }
    HopResult {
        total_score,
        skips: total_skips,
        played: total_played,
    }
}

extern crate alloc;
use alloc::vec::Vec;

struct RevStatesCallback<'a, R: RngCore, T: TerminateSignal> {
    his: Vec<Move>,
    rng: &'a mut R,
    n_times: usize,
    limit: usize,
    sign: &'a T,
    res: Vec<(Vec<Move>, HopResult)>,
}

impl<R: RngCore, T: TerminateSignal> Callback for RevStatesCallback<'_, R, T> {
    type Pruner = FullPruner;

    fn on_win(&mut self, _: &Solitaire) -> Control {
        self.res.push((self.his.clone(), SURE_WIN));
        Control::Halt
    }

    fn on_do_move(
        &mut self,
        g: &Solitaire,
        m: Move,
        _: Encode,
        prune_info: &FullPruner,
    ) -> Control {
        self.his.push(m);
        let rev = prune_info.rev_move();
        // if rev.is_none() && (matches!(m, Move::Reveal(_)) || matches!(m, Move::PileStack(_))) {
        if rev.is_none() {
            self.res.push((
                self.his.clone(),
                hop_solve_game(
                    g,
                    m,
                    self.rng,
                    self.n_times,
                    self.limit,
                    self.sign,
                    prune_info,
                ),
            ));
            Control::Skip
        } else {
            Control::Ok
        }
    }

    fn on_undo_move(&mut self, _: Move, _: Encode, _: &Control) {
        self.his.pop();
    }
}

pub fn list_moves<R: RngCore, T: TerminateSignal>(
    g: &mut Solitaire,
    rng: &mut R,
    n_times: usize,
    limit: usize,
    sign: &T,
) -> Vec<(Vec<Move>, HopResult)> {
    let mut callback = RevStatesCallback {
        his: Vec::default(),
        rng,
        n_times,
        limit,
        sign,
        res: Vec::default(),
    };

    let mut tp = TpTable::default();
    traverse(g, FullPruner::default(), &mut tp, &mut callback);
    callback.res
}
