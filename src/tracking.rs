use core::cell::Cell;

pub trait SearchStatistics {
    fn hit_a_state(&self, depth: usize);
    fn hit_unique_state(&self, depth: usize, n_moves: u32);
    fn finish_move(&self, depth: usize);
}

pub struct EmptySearchStats;

impl SearchStatistics for EmptySearchStats {
    fn hit_a_state(&self, _: usize) {}
    fn hit_unique_state(&self, _: usize, _: u32) {}
    fn finish_move(&self, _: usize) {}
}

pub trait TerminateSignal {
    fn terminate(&self) {}
    fn is_terminated(&self) -> bool {
        false
    }
}

pub struct DefaultTerminateSignal;

impl TerminateSignal for DefaultTerminateSignal {}

pub struct BudgetedTerminateSignal {
    count: Cell<usize>,
    budget: usize,
}

impl BudgetedTerminateSignal {
    pub fn new(budget: usize) -> Self {
        Self {
            count: Cell::new(0),
            budget,
        }
    }
}

impl TerminateSignal for BudgetedTerminateSignal {
    fn is_terminated(&self) -> bool {
        let c = self.count.get() + 1;
        self.count.set(c);
        c > self.budget
    }
}
