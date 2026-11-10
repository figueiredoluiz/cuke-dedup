use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, RngSeed, TestCaseResult, TestRunner};
use std::cell::{Cell, RefCell};

/// Runs a fixed bounded search and prints concrete original/minimized inputs on failure.
pub(crate) fn generated<S: Strategy>(strategy: S, check: impl Fn(&S::Value) -> TestCaseResult)
where
    S::Value: Clone,
{
    let config = Config {
        cases: 256,
        max_shrink_iters: 256,
        max_shrink_time: 1_000,
        max_local_rejects: 128,
        max_global_rejects: 128,
        failure_persistence: None,
        rng_algorithm: RngAlgorithm::ChaCha,
        rng_seed: RngSeed::Fixed(42),
        ..Config::default()
    };
    let first_failure = RefCell::new(None);
    let evaluations = Cell::new(0usize);
    let result = TestRunner::new(config).run(&strategy, |input| {
        evaluations.set(evaluations.get() + 1);
        // Proptest catches checker panics outside this closure, so retain the attempt first.
        let first_candidate = first_failure.borrow().is_none();
        if first_candidate {
            *first_failure.borrow_mut() = Some(input.clone());
        }
        let result = check(&input);
        if first_candidate && result.is_ok() {
            *first_failure.borrow_mut() = None;
        }
        result
    });
    if let Err(error) = result {
        panic!(
            "ChaCha seed 42; replay concrete inputs with the failing checker:\noriginal: {:?}\n{error}",
            first_failure.into_inner()
        );
    }
    assert_eq!(evaluations.get(), 256, "all generated cases must execute");
}
