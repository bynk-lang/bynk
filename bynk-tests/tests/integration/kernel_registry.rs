//! The enumerable kernel-method registry (ADR 0063). Its tables are generated
//! from the typed `KernelOp` declaration (#3164), and the checker matches on
//! `KernelOp` exhaustively, so the registry cannot list a method the checker
//! rejects or omit one it accepts: both are compile errors, not test failures.
//! What a type cannot say is that a display signature leads with its method's
//! name, which is checked here.

use bynk_check::kernel_methods::{
    EFFECT_RESULT_METHODS, FLOAT_METHODS, INT_METHODS, LIST_METHODS, MAP_METHODS, OPTION_METHODS,
    QUERY_METHODS, RESULT_METHODS, STRING_METHODS,
};

#[test]
fn registries_are_well_formed() {
    for methods in [
        INT_METHODS,
        FLOAT_METHODS,
        STRING_METHODS,
        LIST_METHODS,
        MAP_METHODS,
        OPTION_METHODS,
        RESULT_METHODS,
        EFFECT_RESULT_METHODS,
        QUERY_METHODS,
    ] {
        assert!(!methods.is_empty());
        for meth in methods {
            assert!(!meth.name.is_empty());
            // The signature begins with the method name (a `name(...) -> T`).
            assert!(
                meth.signature.starts_with(meth.name),
                "signature {:?} should lead with {:?}",
                meth.signature,
                meth.name
            );
        }
    }
}
