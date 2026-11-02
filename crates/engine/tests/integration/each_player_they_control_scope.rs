//! Runtime + AST regression: "each player … [count] of [X] THEY control" must
//! bind the per-player count to the ITERATING player, not the caster.
//!
//! Three cards exhibit the bug:
//!   - Acidic Soil — "deals damage to each player equal to the number of lands
//!     they control" (parser-only; `DamageEachPlayer` already resolves per player).
//!   - Biorhythm + Shaman of Forgotten Ways — "Each player's life total becomes
//!     the number of creatures they control" (parser + `SetLifeTotal` resolver).
//!
//! Before the fix the count carried `controller: You`, so every player took /
//! was set to the CASTER's count. After the fix the count carries
//! `ControllerRef::ScopedPlayer` and the resolver rebinds `scoped_player` per
//! recipient.
//!
//! This file also carries the issue #10455 production-path regression coverage:
//! a `SetLifeTotal { target: AllPlayers }` clause is a NON-targeted mass effect
//! (no "target [something]" phrase in the Oracle text — CR 115.1a), so casting
//! / triggering / activating it must declare zero target slots and resolve by
//! iterating every player in APNAP order. Before the fix the slot authority
//! minted a required slot whose legal set is always empty, so every such spell
//! was rejected at announcement with "No legal targets available" and every
//! such trigger was dropped before it could reach the stack.
//!
//! CR 109.5: "they/their control" in an each-player effect refers to the player
//! the effect is being applied to.
//! CR 115.1a: a spell is targeted only if it uses the phrase "target [something]".
//! CR 119.5: a "becomes [N]" effect sets each affected player's life total.
//! CR 120.3: damage to each player is dealt to each player in turn.

use engine::game::scenario::{GameScenario, P0, P1};
use engine::parser::oracle_effect::parse_effect_chain;
use engine::types::ability::{
    AbilityKind, ControllerRef, Effect, QuantityExpr, QuantityRef, TargetFilter,
};
use engine::types::format::FormatConfig;
use engine::types::identifiers::ObjectId;
use engine::types::mana::{ManaColor, ManaCost, ManaType, ManaUnit};
use engine::types::phase::Phase;
use engine::types::player::PlayerId;
use engine::types::zones::Zone;

const ACIDIC_SOIL: &str =
    "Acidic Soil deals damage to each player equal to the number of lands they control.";
const BIORHYTHM: &str = "Each player's life total becomes the number of creatures they control.";
const SHAMAN_ACTIVATED: &str = "Each player's life total becomes the number of creatures they \
     control. Activate only if creatures you control have total power 8 or greater.";

// --- Regression baselines (must stay UNSCOPED) --------------------------------
const WORLDFIRE_LIFE: &str = "Each player's life total becomes 1.";
const REPAY_IN_KIND: &str =
    "Each player's life total becomes the lowest life total among all players.";
const OKETRA: &str = "Your life total becomes equal to your starting life total.";

// --- Issue #10455 production-path coverage: full Oracle texts ------------------
const WORLDFIRE_FULL: &str = "Exile all permanents. Exile all cards from all hands and \
     graveyards. Each player's life total becomes 1.";
const ARBITER_OF_KNOLLRIDGE: &str = "Vigilance\nWhen this creature enters, each player's life \
     total becomes the highest life total among all players.";
const THE_DARKHOLD: &str = "{T}: Put a sin counter on The Darkhold. Draw a card for each sin \
     counter on it, then you lose 1 life for each card in your hand.\n\
     {6}{B}{B}, {T}: Each player's life total becomes the lowest life total among all players. \
     Activate only as a sorcery.";

/// Extract the `controller` of the lone `ObjectCount` quantity in `amount`.
fn object_count_controller(amount: &QuantityExpr) -> Option<ControllerRef> {
    match amount {
        QuantityExpr::Ref {
            qty:
                QuantityRef::ObjectCount {
                    filter: TargetFilter::Typed(tf),
                },
        } => tf.controller.clone(),
        _ => None,
    }
}

// =============================== AST tests ====================================

/// CR 109.5: Acidic Soil's per-player land count binds to `ScopedPlayer`.
#[test]
fn acidic_soil_count_is_scoped_player() {
    let def = parse_effect_chain(ACIDIC_SOIL, AbilityKind::Spell);
    match &*def.effect {
        Effect::DamageEachPlayer { amount, .. } => {
            assert_eq!(
                object_count_controller(amount),
                Some(ControllerRef::ScopedPlayer),
                "land count must scope to the iterating player, not the caster (You)"
            );
        }
        other => panic!("expected DamageEachPlayer, got {other:?}"),
    }
}

/// CR 109.5 + CR 119.5: Biorhythm's per-player creature count binds to `ScopedPlayer`.
#[test]
fn biorhythm_count_is_scoped_player() {
    let def = parse_effect_chain(BIORHYTHM, AbilityKind::Spell);
    match &*def.effect {
        Effect::SetLifeTotal { target, amount } => {
            assert_eq!(
                *target,
                TargetFilter::AllPlayers,
                "each player's life total"
            );
            assert_eq!(
                object_count_controller(amount),
                Some(ControllerRef::ScopedPlayer),
                "creature count must scope to the iterating player, not the caster (You)"
            );
        }
        other => panic!("expected SetLifeTotal, got {other:?}"),
    }
}

/// Shaman of Forgotten Ways: the SetLifeTotal amount scopes per player even when
/// the activation-condition clause ("Activate only if creatures you control have
/// total power 8 or greater") trails the effect body — the "you control"
/// condition does NOT bleed into the count's scope. The activation condition
/// itself is parsed by the ability-level parser (not `parse_effect_chain`) and
/// resolved by `resolve_quantity_for_ability_condition`, a separate caster-scoped
/// path untouched by this fix.
#[test]
fn shaman_set_life_amount_is_scoped_player() {
    let def = parse_effect_chain(SHAMAN_ACTIVATED, AbilityKind::Activated);
    match &*def.effect {
        Effect::SetLifeTotal { target, amount } => {
            assert_eq!(*target, TargetFilter::AllPlayers);
            assert_eq!(
                object_count_controller(amount),
                Some(ControllerRef::ScopedPlayer),
                "creature count must scope per player, not to the caster"
            );
        }
        other => panic!("expected SetLifeTotal, got {other:?}"),
    }
}

// --- No-regression AST baselines ---------------------------------------------

/// Worldfire: numeric "becomes 1" stays a `Fixed` literal (no scope to apply).
#[test]
fn worldfire_stays_fixed_one() {
    let def = parse_effect_chain(WORLDFIRE_LIFE, AbilityKind::Spell);
    match &*def.effect {
        Effect::SetLifeTotal { target, amount } => {
            assert_eq!(*target, TargetFilter::AllPlayers);
            assert!(
                matches!(amount, QuantityExpr::Fixed { value: 1 }),
                "Worldfire must stay Fixed{{1}}, got {amount:?}"
            );
        }
        other => panic!("expected SetLifeTotal, got {other:?}"),
    }
}

/// Repay in Kind: cross-player extremum stays `LifeTotal{AllPlayers{Min}}` — it
/// carries no "they control" count, so the per-player scope must not corrupt it.
#[test]
fn repay_in_kind_stays_cross_player_extremum() {
    let def = parse_effect_chain(REPAY_IN_KIND, AbilityKind::Spell);
    match &*def.effect {
        Effect::SetLifeTotal { target, amount } => {
            assert_eq!(*target, TargetFilter::AllPlayers);
            // No ObjectCount → controller scope is irrelevant; must NOT be an
            // ObjectCount at all (it's a cross-player life extremum).
            assert!(
                object_count_controller(amount).is_none(),
                "Repay in Kind must stay a cross-player extremum, not an ObjectCount, got {amount:?}"
            );
        }
        other => panic!("expected SetLifeTotal, got {other:?}"),
    }
}

/// Oketra's Last Mercy: "your life total becomes …" stays Controller/You, not
/// AllPlayers and not ScopedPlayer.
#[test]
fn oketra_stays_controller_scoped() {
    let def = parse_effect_chain(OKETRA, AbilityKind::Spell);
    match &*def.effect {
        Effect::SetLifeTotal { target, .. } => {
            assert_ne!(
                *target,
                TargetFilter::AllPlayers,
                "your life total is single-player (Controller), not AllPlayers"
            );
        }
        other => panic!("expected SetLifeTotal, got {other:?}"),
    }
}

// ============================= Runtime tests ==================================

/// Acidic Soil runtime: P0 controls 5 lands, P1 controls 2. Each player takes
/// damage equal to the number of lands THEY control. Reverting the parser fix
/// (count → `You`) makes BOTH players take the caster's count (5) and fails.
///
/// CR 120.3: each player is dealt the per-player amount.
#[test]
fn acidic_soil_damages_each_player_by_their_own_land_count() {
    let mut scenario = GameScenario::new_n_player(2, 42);
    scenario.at_phase(Phase::PreCombatMain);

    // P0 (caster) controls 5 lands, P1 controls 2 (A != B).
    for _ in 0..5 {
        scenario.add_basic_land(P0, engine::types::mana::ManaColor::Red);
    }
    for _ in 0..2 {
        scenario.add_basic_land(P1, engine::types::mana::ManaColor::Green);
    }

    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "Acidic Soil", false, ACIDIC_SOIL)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    let outcome = runner.cast(spell).resolve();

    // P0 takes 5 (its own lands), P1 takes 2 (its own lands).
    outcome.assert_life_delta(P0, -5);
    outcome.assert_life_delta(P1, -2);
}

/// Biorhythm runtime: P0 controls 3 creatures, P1 controls 1. Each player's life
/// total becomes the number of creatures THEY control. Reverting the parser fix
/// (count → `You`) OR the resolver fix (amount resolved once, pre-loop, from the
/// caster's perspective) sets BOTH players to the caster's count (3) and fails.
///
/// Production path (issue #10455): the parsed Biorhythm is a non-targeted
/// `SetLifeTotal { target: AllPlayers }` spell — the cast declares zero target
/// slots (CR 115.1a) and resolution iterates every player in APNAP order with
/// the per-player `ScopedPlayer` creature count, so the PARSED effect is
/// exercised end-to-end through `cast(..).resolve()` instead of a direct
/// resolver call.
///
/// CR 119.5: life total is set to the per-player creature count.
#[test]
fn biorhythm_sets_each_player_life_to_their_own_creature_count() {
    let mut scenario = GameScenario::new_n_player(2, 42);
    scenario.at_phase(Phase::PreCombatMain);

    // P0 (caster) controls 3 creatures, P1 controls 1 (C != D).
    for _ in 0..3 {
        scenario.add_vanilla(P0, 1, 1);
    }
    scenario.add_vanilla(P1, 1, 1);

    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "Biorhythm", false, BIORHYTHM)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    runner.cast(spell).resolve();

    // P0 life -> 3 (its own creatures), P1 life -> 1 (its own creature).
    assert_eq!(
        runner.life(P0),
        3,
        "P0 life set to its own creature count (3)"
    );
    assert_eq!(
        runner.life(P1),
        1,
        "P1 life set to its own creature count (1)"
    );
}

/// CR 904.13b + CR 810.9d + CR 119.5, runtime: Repay in Kind's parsed
/// cross-player extremum resolves in Archenemy Commander with the archenemy
/// in seat 1. The Min over team totals is the archenemy's 10, not the
/// heroes' shared 60, so every side is set to 10. Reverting the seat-1
/// dedup-key fix collapses the archenemy into the heroes' key and reads 60:
/// the archenemy would rise to 60 and fail below.
///
/// Production path (issue #10455): the spell is cast through
/// `cast(..).resolve()` — a non-targeted `SetLifeTotal { target: AllPlayers }`
/// declares zero slots at announcement (CR 115.1a) and the cross-player Min
/// is computed at resolution. The archenemy (seat 1) is the active player —
/// CR 904.6: the archenemy takes the first turn — so it holds and casts the
/// spell; the cross-player effect is caster-independent, so the seat split
/// under test is unaffected.
#[test]
fn repay_in_kind_seat_one_archenemy_sets_all_sides_to_ten() {
    let mut config = FormatConfig::archenemy_commander();
    config.archenemy_player = Some(PlayerId(1));
    let mut scenario = GameScenario::new_with_format(config, 4, 42);
    scenario.at_phase(Phase::PreCombatMain);

    // The archenemy is seat 1 and therefore the active player (CR 904.6).
    let spell = scenario
        .add_spell_to_hand_from_oracle(P1, "Repay in Kind", false, REPAY_IN_KIND)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();

    // Archenemy at 10; heroes splitting a shared 60.
    runner.state_mut().players[1].life = 10;
    runner.state_mut().players[0].life = 20;
    runner.state_mut().players[2].life = 20;
    runner.state_mut().players[3].life = 20;

    runner.cast(spell).resolve();

    let state = runner.state();
    assert_eq!(
        state.players[1].life, 10,
        "seat-1 archenemy stays at the minimum 10 (a collapsed key reads 60)"
    );
    let heroes_shared: i32 = [0, 2, 3].iter().map(|&i| state.players[i].life).sum();
    assert_eq!(
        heroes_shared, 10,
        "the heroes' shared total is set to the minimum 10"
    );
}

// ================ Issue #10455: production-path regression tests ==============
//
// Each test below failed before the `extract_target_filter_from_effect`
// carve-out: the spell/activation was rejected at announcement with "No legal
// targets available" (cast/activation) or the trigger was dropped before
// reaching the stack (Arbiter). All exercise the full production pipeline.

/// CR 115.1a + CR 119.5, issue #10455: Worldfire is a NON-targeted mass spell.
/// Its chain is `ChangeZoneAll(permanents)` → `ChangeZoneAll(hand)` →
/// `Unimplemented("graveyards", a soft no-op)` → `SetLifeTotal { target:
/// AllPlayers, amount: 1 }`. Before the fix the final clause minted a required
/// slot with an empty legal set and the whole cast was rejected at
/// announcement; after it the cast declares zero targets and resolves every
/// clause: the board and hands are exiled and each player's life total becomes
/// 1.
#[test]
fn worldfire_cast_exiles_every_permanent_and_sets_every_life_to_one() {
    let mut scenario = GameScenario::new_n_player(2, 42);
    scenario.at_phase(Phase::PreCombatMain);

    scenario.add_vanilla(P0, 2, 2);
    scenario.add_basic_land(P0, ManaColor::Green);
    scenario.add_vanilla(P1, 3, 3);
    let p1_in_hand = scenario.add_card_to_hand(P1, "P1 Card");

    let spell = scenario
        .add_spell_to_hand_from_oracle(P0, "Worldfire", false, WORLDFIRE_FULL)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    let outcome = runner.cast(spell).resolve();

    // CR 119.5: the SetLifeTotal{AllPlayers} clause sets both players to 1.
    assert_eq!(runner.life(P0), 1, "P0 life total becomes 1");
    assert_eq!(runner.life(P1), 1, "P1 life total becomes 1");
    // "Exile all permanents": both battlefields are empty.
    assert_eq!(runner.battlefield_count(P0), 0, "P0 battlefield is empty");
    assert_eq!(runner.battlefield_count(P1), 0, "P1 battlefield is empty");
    // "Exile all cards from all hands": P1's card is exiled (the graveyard
    // sub-clause is an unimplemented soft no-op, so nothing leaves the grave).
    outcome.assert_zone(&[p1_in_hand], Zone::Exile);
}

/// CR 115.1a + CR 119.5, issue #10455: Arbiter of Knollridge's ETB trigger is a
/// non-targeted `SetLifeTotal { target: AllPlayers }` whose amount is the
/// HIGHEST life total among all players. Before the fix the trigger's target
/// slots were built with an unsatisfiable required slot and the trigger was
/// dropped before ever reaching the stack, leaving both life totals untouched;
/// after the fix it goes on the stack untargeted and resolves, setting each
/// player to the maximum (15 here).
#[test]
fn arbiter_etb_trigger_sets_each_player_to_the_highest_life_total() {
    let mut scenario = GameScenario::new_n_player(2, 42);
    scenario.at_phase(Phase::PreCombatMain);
    scenario.with_life(P0, 10);
    scenario.with_life(P1, 15);

    let arbiter = scenario
        .add_creature_to_hand_from_oracle(P0, "Arbiter of Knollridge", 5, 5, ARBITER_OF_KNOLLRIDGE)
        .with_mana_cost(ManaCost::zero())
        .id();

    let mut runner = scenario.build();
    runner.cast(arbiter).resolve();

    // Max of {10, 15} is 15 — both players are set to it at trigger resolution.
    assert_eq!(
        runner.life(P0),
        15,
        "P0 is set to the highest life total (15)"
    );
    assert_eq!(
        runner.life(P1),
        15,
        "P1 is set to the highest life total (15)"
    );
}

/// CR 115.1a + CR 119.5, issue #10455: The Darkhold's {6}{B}{B}, {T} activation
/// ("Activate only as a sorcery") is a non-targeted `SetLifeTotal { target:
/// AllPlayers }` whose amount is the LOWEST life total among all players.
/// Before the fix the activation was rejected at announcement with "No legal
/// targets available"; after the fix it pays its cost, goes on the stack
/// untargeted, and resolves, setting each player to the minimum (10 here).
#[test]
fn darkhold_sorcery_activation_sets_each_player_to_the_lowest_life_total() {
    let mut scenario = GameScenario::new_n_player(2, 42);
    scenario.at_phase(Phase::PreCombatMain);
    scenario.with_life(P0, 10);
    scenario.with_life(P1, 20);

    let darkhold = scenario
        .add_artifact_from_oracle(P0, "The Darkhold", THE_DARKHOLD)
        .id();

    // {6}{B}{B}: fund P0's pool with six colorless and two black mana.
    scenario.with_mana_pool(
        P0,
        (0..6)
            .map(|_| ManaUnit::new(ManaType::Colorless, ObjectId(0), false, Vec::new()))
            .chain((0..2).map(|_| ManaUnit::new(ManaType::Black, ObjectId(0), false, Vec::new())))
            .collect(),
    );

    let mut runner = scenario.build();
    // Ability index 1 is the {6}{B}{B} life-set (index 0 is the sin-counter one).
    runner.activate(darkhold, 1).resolve();

    // Min of {10, 20} is 10 — both players end at the minimum.
    assert_eq!(
        runner.life(P0),
        10,
        "P0 stays at the lowest life total (10)"
    );
    assert_eq!(
        runner.life(P1),
        10,
        "P1 is set to the lowest life total (10)"
    );
}
