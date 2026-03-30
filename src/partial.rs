use arrayvec::ArrayVec;
use rand::seq::SliceRandom;
use rand::RngCore;

use crate::card::{Card, N_CARDS, N_RANKS, N_SUITS};
use crate::deck::{Deck, N_DECK_CARDS, N_PILES};
use crate::stack::Stack;
use crate::standard::{HiddenVec, PileVec, StandardSolitaire};
use crate::state::Solitaire;

use core::num::NonZeroU8;

/// What a screen adapter can observe: visible cards + hidden card counts.
pub struct PartialBoard {
    /// Visible (face-up) cards per tableau pile, bottom to top.
    pub pile_cards: [PileVec; N_PILES as usize],
    /// Number of face-down cards in each tableau pile.
    pub hidden_counts: [u8; N_PILES as usize],
    /// Foundation rank count per suit (0 = empty, 13 = complete).
    /// Index order: hearts(0), diamonds(1), clubs(2), spades(3).
    pub foundation: [u8; N_SUITS as usize],
    /// Visible waste cards, bottom to top (top = currently drawable).
    pub waste: ArrayVec<Card, { N_DECK_CARDS as usize }>,
    /// Exact current deck order when known: [hidden waste | visible waste | stock].
    ///
    /// This is especially useful for screen adapters that can cycle the stock and
    /// reconstruct the real draw-3 sequence instead of randomizing the unseen waste.
    pub known_deck_order: Option<ArrayVec<Card, { N_DECK_CARDS as usize }>>,
    /// Number of cards remaining in stock (face-down, undealt).
    pub stock_count: u8,
    /// Draw step (1 or 3).
    pub draw_step: u8,
}

#[derive(Debug, Clone)]
pub enum PartialBoardError {
    /// Known cards don't sum to 52 with unknowns.
    InvalidCardCount { expected: u8, got: u8 },
    /// Duplicate card among known cards.
    DuplicateCard(Card),
    /// Hidden count for pile i exceeds maximum (i).
    InvalidHiddenCount { pile: u8, count: u8, max: u8 },
    /// Foundation count for a suit exceeds 13.
    InvalidFoundation { suit: u8, count: u8 },
    /// Invalid draw step (must be 1 or 3).
    InvalidDrawStep(u8),
    /// Known deck order length doesn't match the number of deck cards in play.
    InvalidDeckOrderLen { expected: u8, got: u8 },
    /// Visible waste disagrees with the supplied exact deck ordering.
    VisibleWasteMismatch,
}

impl PartialBoard {
    /// Validate that this partial board is consistent.
    pub fn validate(&self) -> Result<(), PartialBoardError> {
        if self.draw_step != 1 && self.draw_step != 3 {
            return Err(PartialBoardError::InvalidDrawStep(self.draw_step));
        }

        for (suit, &count) in self.foundation.iter().enumerate() {
            if count > N_RANKS {
                return Err(PartialBoardError::InvalidFoundation {
                    suit: suit as u8,
                    count,
                });
            }
        }

        for (pile, &count) in self.hidden_counts.iter().enumerate() {
            if count > pile as u8 {
                return Err(PartialBoardError::InvalidHiddenCount {
                    pile: pile as u8,
                    count,
                    max: pile as u8,
                });
            }
        }

        let foundation_count: u8 = self.foundation.iter().sum();
        let pile_visible: u8 = self.pile_cards.iter().map(|p| p.len() as u8).sum();
        let hidden_total: u8 = self.hidden_counts.iter().sum();
        let total_deck = N_CARDS - foundation_count - pile_visible - hidden_total;
        let visible_waste = self.waste.len() as u8;
        let waste_invisible = total_deck.saturating_sub(self.stock_count + visible_waste);

        // Check for duplicate known cards
        let mut seen: u64 = 0;
        let mut mark = |card: Card| -> Result<(), PartialBoardError> {
            let m = card.mask();
            if seen & m != 0 {
                return Err(PartialBoardError::DuplicateCard(card));
            }
            seen |= m;
            Ok(())
        };

        // Foundation cards
        for suit in 0..N_SUITS {
            for rank in 0..self.foundation[suit as usize] {
                mark(Card::new(rank, suit))?;
            }
        }

        // Visible pile cards
        for pile in &self.pile_cards {
            for &card in pile {
                mark(card)?;
            }
        }

        if let Some(deck_order) = &self.known_deck_order {
            let got = deck_order.len() as u8;
            if got != total_deck {
                return Err(PartialBoardError::InvalidDeckOrderLen {
                    expected: total_deck,
                    got,
                });
            }

            let visible_start = waste_invisible as usize;
            let visible_end = visible_start + self.waste.len();
            if deck_order
                .get(visible_start..visible_end)
                .filter(|cards| *cards == self.waste.as_slice())
                .is_none()
            {
                return Err(PartialBoardError::VisibleWasteMismatch);
            }

            for &card in deck_order {
                mark(card)?;
            }
        } else {
            // Visible waste cards
            for &card in &self.waste {
                mark(card)?;
            }
        }

        // Total card count check
        let known_count = seen.count_ones() as u8;
        let actual_unknown = if self.known_deck_order.is_some() {
            hidden_total
        } else {
            hidden_total + self.stock_count + waste_invisible
        };

        if known_count + actual_unknown != N_CARDS {
            return Err(PartialBoardError::InvalidCardCount {
                expected: N_CARDS,
                got: known_count + actual_unknown,
            });
        }

        Ok(())
    }

    /// Convert this partial board into a `Solitaire` by randomly filling unknown cards.
    ///
    /// Unknown positions (hidden tableau cards, non-visible waste, stock) are filled
    /// with the remaining cards from the 52-card deck, shuffled randomly.
    pub fn to_solitaire<R: RngCore>(&self, rng: &mut R) -> Solitaire {
        Solitaire::from(&self.to_standard_solitaire(rng))
    }

    /// Convert this partial board into a `StandardSolitaire` by randomly filling unknowns.
    pub fn to_standard_solitaire<R: RngCore>(&self, rng: &mut R) -> StandardSolitaire {
        // 1. Collect all known cards into a mask
        let mut known_mask: u64 = 0;

        for suit in 0..N_SUITS {
            for rank in 0..self.foundation[suit as usize] {
                known_mask |= Card::new(rank, suit).mask();
            }
        }
        for pile in &self.pile_cards {
            for &card in pile {
                known_mask |= card.mask();
            }
        }
        if let Some(deck_order) = &self.known_deck_order {
            for &card in deck_order {
                known_mask |= card.mask();
            }
        } else {
            for &card in &self.waste {
                known_mask |= card.mask();
            }
        }

        // 2. Collect remaining unknown cards and shuffle
        let mut remaining: ArrayVec<Card, { N_CARDS as usize }> = ArrayVec::new();
        for i in 0..N_CARDS {
            let card = Card::from_mask_index(i);
            if known_mask & card.mask() == 0 {
                remaining.push(card);
            }
        }
        remaining.shuffle(rng);

        let mut idx = 0;

        // 3. Fill hidden piles
        let mut hidden_piles: [HiddenVec; N_PILES as usize] = Default::default();
        for (i, pile) in hidden_piles.iter_mut().enumerate() {
            for _ in 0..self.hidden_counts[i] {
                pile.push(remaining[idx]);
                idx += 1;
            }
        }

        // 4. Piles (visible cards, copied directly)
        let piles = self.pile_cards.clone();

        // 5. Build deck: [non-visible waste | visible waste | stock]
        let foundation_count: u8 = self.foundation.iter().sum();
        let pile_visible: u8 = self.pile_cards.iter().map(|p| p.len() as u8).sum();
        let hidden_total: u8 = self.hidden_counts.iter().sum();
        let total_deck_cards = N_CARDS - foundation_count - pile_visible - hidden_total;
        let mut deck_cards: ArrayVec<Card, { N_DECK_CARDS as usize }> = ArrayVec::new();
        let draw_cur = if let Some(deck_order) = &self.known_deck_order {
            deck_cards.try_extend_from_slice(deck_order).unwrap();
            total_deck_cards - self.stock_count
        } else {
            let visible_waste = self.waste.len() as u8;
            let waste_invisible = total_deck_cards.saturating_sub(self.stock_count + visible_waste);

            // Non-visible waste (unknown, filled from remaining)
            for _ in 0..waste_invisible {
                deck_cards.push(remaining[idx]);
                idx += 1;
            }

            // Visible waste
            for &card in &self.waste {
                deck_cards.push(card);
            }

            let draw_cur = deck_cards.len() as u8;

            // Stock (unknown, filled from remaining)
            for _ in 0..self.stock_count {
                deck_cards.push(remaining[idx]);
                idx += 1;
            }

            draw_cur
        };

        let draw_step = NonZeroU8::new(self.draw_step).unwrap();
        let deck = Deck::from_cards(&deck_cards, draw_cur, draw_step);

        // 6. Foundation
        let stack = Stack::from_counts(self.foundation);

        StandardSolitaire::from_parts(hidden_piles, piles, deck, stack)
    }
}

/// Parse a card string like "Ah", "10C", "KS" into a `Card`.
/// Rank: A, 2-10, J, Q, K. Suit: H/h, D/d, C/c, S/s.
pub fn parse_card(s: &str) -> Option<Card> {
    let s = s.trim();
    if s.len() < 2 {
        return None;
    }

    let (rank_str, suit_char) = s.split_at(s.len() - 1);
    let suit_char = suit_char.chars().next()?;

    let suit: u8 = match suit_char.to_ascii_lowercase() {
        'h' => 0,
        'd' => 1,
        'c' => 2,
        's' => 3,
        _ => return None,
    };

    let rank: u8 = match rank_str {
        "A" | "a" => 0,
        "2" => 1,
        "3" => 2,
        "4" => 3,
        "5" => 4,
        "6" => 5,
        "7" => 6,
        "8" => 7,
        "9" => 8,
        "10" => 9,
        "J" | "j" => 10,
        "Q" | "q" => 11,
        "K" | "k" => 12,
        _ => return None,
    };

    Some(Card::new(rank, suit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shuffler::default_shuffle;

    #[test]
    fn test_parse_card() {
        assert_eq!(parse_card("Ah").unwrap().split(), (0, 0));
        assert_eq!(parse_card("KS").unwrap().split(), (12, 3));
        assert_eq!(parse_card("10C").unwrap().split(), (9, 2));
        assert_eq!(parse_card("2d").unwrap().split(), (1, 1));
        assert!(parse_card("Xz").is_none());
    }

    #[test]
    fn test_round_trip() {
        use rand::SeedableRng;
        use rand::rngs::SmallRng;

        // Create a known game from a seed
        let cards = default_shuffle(42);
        let draw_step = NonZeroU8::new(3).unwrap();
        let std_game = StandardSolitaire::new(&cards, draw_step);

        // Extract visible state into a PartialBoard
        let board = PartialBoard {
            pile_cards: std_game.get_piles().clone(),
            hidden_counts: core::array::from_fn(|i| std_game.get_hidden()[i].len() as u8),
            foundation: [0; 4], // new game, no foundation
            waste: ArrayVec::new(),
            known_deck_order: None,
            stock_count: N_DECK_CARDS, // all 24 cards in stock
            draw_step: 3,
        };

        // Validate
        board.validate().unwrap();

        // Convert to Solitaire and verify solver can work with it
        let mut rng = SmallRng::seed_from_u64(123);
        let solitaire = board.to_solitaire(&mut rng);

        // Basic sanity: stack should be empty, not a win
        assert!(!solitaire.is_win());
        assert_eq!(solitaire.get_stack().len(), 0);

        // Should be able to generate moves
        let moves = solitaire.gen_moves::<false>();
        // A new solitaire game should have some available moves
        assert!(!moves.is_empty());
    }

    #[test]
    fn test_partial_board_with_progress() {
        use rand::SeedableRng;
        use rand::rngs::SmallRng;

        // Simulate a mid-game board where some cards are on foundation
        let mut board = PartialBoard {
            pile_cards: Default::default(),
            hidden_counts: [0; 7],
            foundation: [1, 0, 0, 0], // Ace of hearts on foundation
            waste: ArrayVec::new(),
            known_deck_order: None,
            stock_count: 0,
            draw_step: 1,
        };

        // Put remaining cards as visible pile cards (contrived but valid)
        // 51 remaining cards across 7 piles
        let mut card_idx = 0u8;
        for pile in 0..7 {
            for _ in 0..7 {
                // Skip Ah (rank=0, suit=0) which is on foundation
                loop {
                    let card = Card::new(card_idx / N_SUITS, card_idx % N_SUITS);
                    card_idx += 1;
                    if card.rank() == 0 && card.suit() == 0 {
                        continue;
                    }
                    board.pile_cards[pile].push(card);
                    break;
                }
            }
        }
        // Remaining cards go to stock
        let mut stock = 0u8;
        while card_idx < N_CARDS {
            let _card = Card::new(card_idx / N_SUITS, card_idx % N_SUITS);
            card_idx += 1;
            stock += 1;
        }
        board.stock_count = stock;

        board.validate().unwrap();

        let mut rng = SmallRng::seed_from_u64(999);
        let solitaire = board.to_solitaire(&mut rng);
        assert_eq!(solitaire.get_stack().get(0), 1); // hearts has 1 card
    }

    #[test]
    fn test_known_deck_order_round_trip() {
        use rand::SeedableRng;

        let cards = default_shuffle(7);
        let draw_step = NonZeroU8::new(3).unwrap();
        let mut std_game = StandardSolitaire::new(&cards, draw_step);
        std_game.do_move(&crate::standard::StandardMove::DRAW_NEXT).unwrap();
        std_game.do_move(&crate::standard::StandardMove::DRAW_NEXT).unwrap();

        let waste: ArrayVec<Card, { N_DECK_CARDS as usize }> =
            std_game.get_deck().waste_iter().collect();
        let deck_order: ArrayVec<Card, { N_DECK_CARDS as usize }> =
            std_game.get_deck().iter().collect();

        let board = PartialBoard {
            pile_cards: std_game.get_piles().clone(),
            hidden_counts: core::array::from_fn(|i| std_game.get_hidden()[i].len() as u8),
            foundation: [0; 4],
            waste,
            known_deck_order: Some(deck_order.clone()),
            stock_count: std_game.get_deck().deck_iter().len() as u8,
            draw_step: 3,
        };

        board.validate().unwrap();

        let mut rng = rand::rngs::SmallRng::seed_from_u64(1);
        let rebuilt = board.to_standard_solitaire(&mut rng);

        let rebuilt_order: ArrayVec<Card, { N_DECK_CARDS as usize }> =
            rebuilt.get_deck().iter().collect();
        let rebuilt_waste: ArrayVec<Card, { N_DECK_CARDS as usize }> =
            rebuilt.get_deck().waste_iter().collect();

        assert_eq!(rebuilt_order, deck_order);
        assert_eq!(rebuilt_waste, board.waste);
        assert_eq!(rebuilt.get_deck().peek_current(), std_game.get_deck().peek_current());
    }
}
