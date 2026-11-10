use super::*;
use crate::tests::property_runner;
use proptest::prelude::*;
use proptest::test_runner::TestCaseResult;

#[derive(Clone, Debug)]
enum Op {
    Group(usize, usize),
    Payload(Vec<usize>),
    Possible(usize),
}

/// Independently models checked reservations with mathematical integers and atomic refusal.
fn check((limits, ops): &([usize; 3], Vec<Op>)) -> TestCaseResult {
    let mut actual = AmbiguityBudget::new(limits[0], limits[1], limits[2]);
    let mut remaining = limits.map(|n| n as u128);
    let maximum = usize::MAX as u128;
    for op in ops {
        let (observed, expected) = match op {
            Op::Group(members, proof) => {
                let slots = 2 * (*members as u128 + *proof as u128);
                let fits = slots <= maximum && slots <= remaining[0] && remaining[1] > 0;
                if fits {
                    remaining[0] -= slots;
                    remaining[1] -= 1;
                }
                (actual.admit_group(*members, *proof), fits)
            }
            Op::Payload(sizes) => {
                let sum: u128 = sizes.iter().map(|n| *n as u128).sum();
                let fits = sum <= maximum && sum <= remaining[2];
                if fits {
                    remaining[2] -= sum;
                }
                (actual.admit_payload(sizes.iter().copied()), fits)
            }
            Op::Possible(members) => (
                actual.can_hold_group(*members),
                2 * *members as u128 <= limits[0] as u128,
            ),
        };
        prop_assert_eq!(observed, expected, "operation {:?}", op);
        prop_assert_eq!(actual.remaining.map(|n| n as u128), remaining);
        prop_assert_eq!(actual.limits, *limits);
    }
    Ok(())
}

/// Includes checked-overflow witnesses without allocating corresponding memberships or payloads.
fn magnitude() -> impl Strategy<Value = usize> {
    prop_oneof![8 => 0usize..=32, 1 => Just(usize::MAX), 1 => Just(usize::MAX / 2 + 1)]
}

/// Compares mixed group/proof/payload admissions and capacity probes against a wider-integer model.
#[test]
fn generated_ambiguity_reservations_are_checked_and_atomic() {
    let ops = prop_oneof![
        (
            prop_oneof![8 => 2usize..=32, 1 => Just(usize::MAX), 1 => Just(usize::MAX / 2 + 1)],
            any::<bool>()
        )
            .prop_map(|(members, proof)| Op::Group(members, if proof { members } else { 0 })),
        proptest::collection::vec(magnitude(), 0..=4).prop_map(Op::Payload),
        magnitude().prop_map(Op::Possible),
    ];
    property_runner::generated(
        (
            proptest::array::uniform3(magnitude()),
            proptest::collection::vec(ops, 0..=32),
        ),
        check,
    );
}

/// Exhausts every short operation trace across five declared interacting allowance triples.
#[test]
fn small_ambiguity_reservations_match_model_exhaustively() {
    let alphabet = [
        Op::Group(2, 0),
        Op::Group(2, 2),
        Op::Group(3, 0),
        Op::Payload(vec![]),
        Op::Payload(vec![1]),
        Op::Payload(vec![1, 1]),
        Op::Possible(2),
        Op::Possible(3),
        Op::Possible(4),
    ];
    let mut cases = 0;
    for limits in [[0, 0, 0], [2, 1, 1], [4, 2, 2], [8, 1, 2], [8, 2, 1]] {
        let traces = std::iter::once(vec![]).chain(alphabet.iter().flat_map(|a| {
            std::iter::once(vec![a.clone()])
                .chain(alphabet.iter().map(move |b| vec![a.clone(), b.clone()]))
        }));
        for trace in traces {
            let input = (limits, trace);
            let result = check(&input);
            assert!(result.is_ok(), "{input:?}: {result:?}");
            cases += 1;
        }
    }
    assert_eq!(cases, 455);
}
