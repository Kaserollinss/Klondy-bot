use std::path::PathBuf;

use clap::Args;
use loneadapter::adapters::solitaire_cash::{ObservedPile, SolitaireCashLayout};
use loneadapter::adapters::solitaire_cash_templates::MatchCandidate;
use loneadapter::adapters::solitaire_cash_macos::{
    DebugOptions, PapayaSolitaireCashRecognizer, RecognitionReport,
};
use loneadapter::AdapterError;

#[derive(Args, Clone)]
pub struct InspectSolitaireCashArgs {
    #[arg(value_name = "PNG", required = true)]
    pub images: Vec<PathBuf>,
    #[arg(long, default_value_os_t = crate::default_solitaire_cash_assets())]
    pub assets: PathBuf,
    #[arg(long)]
    pub debug_dir: Option<PathBuf>,
}

#[derive(Args, Clone)]
pub struct ValidateSolitaireCashTemplatesArgs {
    #[arg(value_name = "PNG", required = true)]
    pub images: Vec<PathBuf>,
    #[arg(long, default_value_os_t = crate::default_solitaire_cash_assets())]
    pub assets: PathBuf,
    #[arg(long)]
    pub debug_dir: Option<PathBuf>,
}

pub fn run_solitaire_cash_inspect(args: &InspectSolitaireCashArgs) -> Result<(), AdapterError> {
    let debug = DebugOptions {
        enabled: true,
        dump_dir: args.debug_dir.clone(),
    };
    let mut recognizer = PapayaSolitaireCashRecognizer::from_asset_dir(&args.assets)?
        .with_debug(debug);
    let layout = SolitaireCashLayout::default();

    for (index, image) in args.images.iter().enumerate() {
        println!("== Screenshot {} ==", index + 1);
        println!("path: {}", image.display());
        let report = recognizer.inspect_png(image, &layout)?;
        print_report(&report);
        println!("foundation: {}", format_foundation(&report.board.foundation));
        println!(
            "waste_visible: {}",
            format_cards(report.board.waste.iter().map(ToString::to_string).collect())
        );
        println!("stock_present: {}", report.board.stock_present);
        if let Some(path) = &report.annotated_path {
            println!("annotated_overlay: {}", path.display());
        }
        for (pile_index, pile) in report.board.piles.iter().enumerate() {
            println!(
                "pile {}: hidden={} visible={}",
                pile_index + 1,
                pile.hidden_count,
                format_pile(pile)
            );
        }
        println!();
    }

    Ok(())
}

pub fn run_solitaire_cash_validate(
    args: &ValidateSolitaireCashTemplatesArgs,
) -> Result<(), AdapterError> {
    let debug = DebugOptions {
        enabled: true,
        dump_dir: args.debug_dir.clone(),
    };
    let mut recognizer = PapayaSolitaireCashRecognizer::from_asset_dir(&args.assets)?
        .with_debug(debug);
    let layout = SolitaireCashLayout::default();

    for image in &args.images {
        let report = recognizer.inspect_png(image, &layout)?;
        let low_confidence = report
            .slots
            .iter()
            .filter(|slot| slot.low_confidence)
            .collect::<Vec<_>>();
        println!("== Template Validation ==");
        println!("path: {}", image.display());
        println!("low_confidence_slots: {}", low_confidence.len());
        if let Some(path) = &report.annotated_path {
            println!("annotated_overlay: {}", path.display());
        }
        for slot in low_confidence {
            println!(
                "{} state={:?} card={:?} rank={} suit={}",
                slot.label,
                slot.state,
                slot.card,
                format_candidates(&slot.rank_candidates),
                format_candidates(&slot.suit_candidates)
            );
        }
        println!();
    }

    Ok(())
}

fn print_report(report: &RecognitionReport) {
    for slot in &report.slots {
        println!(
            "slot {} state={:?} card={:?} low_confidence={} rank={} suit={}",
            slot.label,
            slot.state,
            slot.card,
            slot.low_confidence,
            format_candidates(&slot.rank_candidates),
            format_candidates(&slot.suit_candidates)
        );
    }
}

fn format_candidates(candidates: &[MatchCandidate]) -> String {
    if candidates.is_empty() {
        "[]".into()
    } else {
        format!(
            "[{}]",
            candidates
                .iter()
                .take(3)
                .map(|candidate| format!("{}:{:.3}", candidate.label, candidate.score))
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn format_foundation(foundation: &[u8; 4]) -> String {
    let labels = ["clubs", "diamonds", "hearts", "spades"];
    labels
        .iter()
        .zip(foundation.iter())
        .map(|(label, count)| format!("{label}={count}"))
        .collect::<Vec<_>>()
        .join(", ")
}

fn format_pile(pile: &ObservedPile) -> String {
    if pile.cards.is_empty() {
        "[]".into()
    } else {
        format_cards(pile.cards.iter().map(ToString::to_string).collect())
    }
}

fn format_cards(cards: Vec<String>) -> String {
    if cards.is_empty() {
        "[]".into()
    } else {
        format!("[{}]", cards.join(", "))
    }
}
