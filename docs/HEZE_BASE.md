# Neutral Heze foundation (first commit)

This commit deliberately introduces no EIP-7805 / FOCIL behavior.

Read the change in this order:

1. types/src/nonstandard.rs and config.rs: Phase::Heze, fork version and activation epoch.
2. types/src/heze/: distinct Heze state, block, block-body and bid types.
   At this intermediate commit their wire layout is inherited unchanged from Gloas.
3. types/src/{combined,traits}.rs: Heze decoding, roots and generic accessors.
4. helper_functions/src/fork.rs: a neutral Gloas-to-Heze upgrade that copies the
   inherited fields and changes the fork version.
5. transition_functions/src/heze/ and combined.rs: independent Heze dispatch with
   inherited transition rules.
6. Supporting client/dependency code: compile-time handling of the new phase.
   Live Heze proposal, Engine API and networking remain explicitly unsupported.

The common bid-field projections let generic code inspect both concrete forks.
They are never SSZ objects or signing inputs. Gloas remains its own phase.

The official pinned Heze vectors should still fail at this intermediate stage:
the actual Heze specification already includes EIP feature changes to its schema
and rules. This scaffold is not the final Heze specification.

The following commit adds EIP-7805 consensus behavior and its focused tests.
EIP-8015 and EIP-8365 consensus support is added in a later follow-up commit;
see HEZE_FOCIL.md for the current implementation and validation.
