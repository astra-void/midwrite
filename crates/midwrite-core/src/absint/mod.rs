//! The value layer: bounded abstract domains for interprocedural analysis.
//!
//! See `docs/architecture/value-analysis.md`.
//!
//! * [`value`]: abstract values (bounded atom sets with `Top`), provenance.
//! * [`heap`]: abstract objects per allocation site, strong and weak
//!   updates, escape.
//! * [`env`]: binding → value environments with join and widening.
//! * [`summary`]: inertness and parameter-use summaries, solved SCC by SCC.
//! * [`relation`]: relationships between effects from provenance.
//!
//! None of these consult TypeScript types, and none of them is a model of
//! JavaScript: they track only identities that decide call targets,
//! receivers, effects, provenance and branch facts.

pub mod env;
pub mod heap;
pub mod relation;
pub mod summary;
pub mod value;

pub use env::Env;
pub use heap::{Heap, ObjectKind};
pub use relation::{RelationKind, ValueRelation};
pub use value::{AbsValue, Atom, CapturedValue, Input, Joined, Provenance};

#[cfg(test)]
mod properties {
    //! Property tests with fixed seeds (reproducible): lattice laws of the
    //! value domain, termination of chains, monotonicity of substitution
    //! and heap reads, safety of escape.

    use super::heap::{Heap, ObjectKind};
    use super::summary::{ArgFact, CallFact, FunctionFacts, ParamUse, solve};
    use super::value::{AbsValue, Atom, MAX_ATOMS};
    use crate::ir::Literal;

    /// xorshift64*: deterministic, no dependency.
    struct Rng(u64);

    impl Rng {
        fn next(&mut self) -> u64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
        }

        fn below(&mut self, n: u64) -> u64 {
            self.next() % n
        }
    }

    const SEEDS: [u64; 4] = [0x5eed_0001, 0x5eed_0002, 0xc0ff_ee00, 0x0dd_ba11];

    fn atom(rng: &mut Rng) -> Atom {
        match rng.below(9) {
            0 => Atom::Param {
                index: rng.below(3) as u32,
            },
            1 => Atom::EffectResult {
                effect: rng.below(3) as u32,
            },
            2 => Atom::lit(Literal::Number(rng.below(4).to_string())),
            3 => Atom::Function {
                id: rng.below(3) as u32,
            },
            4 => Atom::This,
            6 => Atom::Closure {
                id: rng.below(3) as u32,
                parameters: 1,
                captures: vec![super::CapturedValue {
                    value: AbsValue::atom(Atom::Param {
                        index: rng.below(3) as u32,
                    }),
                    handle: None,
                    default: None,
                }]
                .into(),
            },
            7 => Atom::ModuleValue {
                initializer: rng.below(3) as u32,
            },
            5 => {
                let base = atom(rng);
                AbsValue::atom(base)
                    .field(["id", "x"][rng.below(2) as usize])
                    .single()
                    .cloned()
                    .unwrap_or(Atom::This)
            }
            _ => Atom::Opaque { id: rng.below(3) },
        }
    }

    fn value(rng: &mut Rng) -> AbsValue {
        if rng.below(12) == 0 {
            return AbsValue::top();
        }
        let n = rng.below(5);
        AbsValue::from_atoms((0..n).map(|_| atom(rng)))
    }

    #[test]
    fn join_is_an_upper_bound_commutative_and_idempotent() {
        for seed in SEEDS {
            let mut rng = Rng(seed);
            for _ in 0..500 {
                let a = value(&mut rng);
                let b = value(&mut rng);
                let ab = a.joined(&b);
                assert!(a.leq(&ab) && b.leq(&ab), "upper bound: {a} {b} {ab}");
                assert_eq!(ab, b.joined(&a), "commutative: {a} {b}");
                assert_eq!(ab.joined(&ab), ab, "idempotent");
                assert!(ab.is_top() || ab.atoms().len() <= MAX_ATOMS);
            }
        }
    }

    #[test]
    fn join_is_associative_up_to_widening() {
        // With a size bound, (a ⊔ b) ⊔ c and a ⊔ (b ⊔ c) are equal unless
        // one of them widened, in which case the other is below it.
        for seed in SEEDS {
            let mut rng = Rng(seed);
            for _ in 0..500 {
                let (a, b, c) = (value(&mut rng), value(&mut rng), value(&mut rng));
                let left = a.joined(&b).joined(&c);
                let right = a.joined(&b.joined(&c));
                if !left.is_top() && !right.is_top() {
                    assert_eq!(left, right);
                } else {
                    assert!(left.leq(&right) || right.leq(&left));
                }
            }
        }
    }

    #[test]
    fn ascending_chains_terminate_within_the_bound() {
        for seed in SEEDS {
            let mut rng = Rng(seed);
            let mut current = AbsValue::bottom();
            let mut changes = 0;
            for _ in 0..10_000 {
                if current.join(&value(&mut rng)).changed() {
                    changes += 1;
                }
            }
            // At most MAX_ATOMS growth steps plus one widening.
            assert!(changes <= MAX_ATOMS + 1, "{changes} changes");
        }
    }

    #[test]
    fn substitution_and_field_reads_are_monotone() {
        for seed in SEEDS {
            let mut rng = Rng(seed);
            let replacement = value(&mut rng);
            let f = |a: &Atom| (a == &Atom::Param { index: 0 }).then(|| replacement.clone());
            for _ in 0..500 {
                let a = value(&mut rng);
                let b = a.joined(&value(&mut rng));
                assert!(a.substitute(&f).leq(&b.substitute(&f)));
                assert!(a.field("id").leq(&b.field("id")));
            }
        }
    }

    #[test]
    fn heap_reads_are_monotone_in_the_heap_join_and_escape_is_safe() {
        for seed in SEEDS {
            let mut rng = Rng(seed);
            for _ in 0..200 {
                let mut left = Heap::default();
                let sites: Vec<u32> = (0..3).collect();
                for &site in &sites {
                    left.allocate(site, ObjectKind::Plain);
                    left.init(site, "x", value(&mut rng));
                }
                let mut right = left.clone();
                for &site in &sites {
                    if rng.below(2) == 0 {
                        right.write(
                            &AbsValue::atom(Atom::Object { id: site }),
                            "x",
                            value(&mut rng),
                        );
                    }
                }
                let mut joined = left.clone();
                joined.join(&right);
                for &site in &sites {
                    let target = AbsValue::atom(Atom::Object { id: site });
                    assert!(left.read(&target, "x").leq(&joined.read(&target, "x")));
                    assert!(right.read(&target, "x").leq(&joined.read(&target, "x")));
                }
                // After an escape, nothing known about the escaped object
                // survives: every read is Top.
                let victim = AbsValue::atom(Atom::Object { id: 0 });
                joined.escape(&victim);
                assert!(joined.read(&victim, "x").is_top());
            }
        }
    }

    #[test]
    fn summary_solution_is_deterministic_and_a_fixpoint() {
        for seed in SEEDS {
            let mut rng = Rng(seed);
            let n = 40;
            let facts: Vec<FunctionFacts> = (0..n)
                .map(|_| FunctionFacts {
                    refuted: (rng.below(8) == 0).then(|| "effect".to_owned()),
                    calls: (0..rng.below(4))
                        .map(|_| CallFact {
                            callee: rng.below(n as u64) as usize,
                            args: (0..rng.below(3))
                                .map(|_| match rng.below(4) {
                                    0 => ArgFact::Other,
                                    1 => ArgFact::Function(rng.below(n as u64) as usize),
                                    2 => ArgFact::Closure {
                                        function: rng.below(n as u64) as usize,
                                        parameters: 0,
                                        captures: vec![ArgFact::Param(rng.below(2) as u32)].into(),
                                    },
                                    _ => ArgFact::Param(rng.below(2) as u32),
                                })
                                .collect(),
                            label: "call()".to_owned(),
                        })
                        .collect(),
                    params: (0..2)
                        .map(|_| ParamUse {
                            invoked: rng.below(5) == 0,
                            awaited: rng.below(5) == 0,
                            escaped: false,
                        })
                        .collect(),
                })
                .collect();
            let names: Vec<String> = (0..n).map(|i| format!("f{i}")).collect();
            let (first, stats) = solve(&facts, &names);
            let (second, _) = solve(&facts, &names);
            assert_eq!(first, second);
            assert_eq!(stats.budget_exhausted, 0);
            // Fixpoint check: an inert function only calls inert functions.
            for (index, fact) in facts.iter().enumerate() {
                if first[index].is_inert() {
                    assert!(fact.refuted.is_none());
                    for call in &fact.calls {
                        assert!(
                            first[call.callee].is_inert(),
                            "f{index} -> f{}",
                            call.callee
                        );
                    }
                }
            }
        }
    }
}
