# Heze base for the first assignment

This branch starts at Subhasish-Behera/grandine Subh/heze2 commit
c92f155458c12c08207ccb56f29ecbb6b6f952b7. Its FOCIL implementation,
inclusion-list networking, and Engine API integration are retained.

The consensus target is ethereum/consensus-specs commit
12c8b24bc09b12f62688d439ba20ef00b1fdfe1f (the teacher's reference vectors).
The base already implements EIP-7805. This change adds the remaining Heze
consensus changes: EIP-8015 removes retired Eth1/deposit fields using SSZ
progressive-container gaps; EIP-8365 rejects creation of new BLS-withdrawal
validators while allowing deposits topping up existing validators.

Heze is a distinct Phase::Heze and uses Heze state and block containers.
The Gloas-to-Heze upgrade checks completion of the Eth1 deposit bridge.
Genesis assembles legacy deposits before upgrading the final state to Heze.

Run bash scripts/test_heze.sh (or make test-heze) for focused official
Heze vectors and schema/genesis regressions. The source revision, official pytest selectors and fixture checksums are under
testing/reference-vectors; generated binary fixtures are ignored by Git. The Gloas comparison cases check that the
earlier fork still accepts its legacy behavior.

This target is not a claim of heze-devnet-3 or live interoperability.
Quick Slots / EIP-8198 and a multi-client network are outside this assignment.


Eth1 bridge access uses field-based capabilities (Eth1DepositState,
DepositRequestsStartIndexState and BlockBodyWithEth1Deposits). Existing
Phase0-through-Gloas state/container definitions have no dependency on a
future Heze-named trait.

Before direct Cargo builds in a fresh checkout, initialize submodules with
git submodule update --init --recursive, then run
bash scripts/prepare_eip8321_dependencies.sh. Make runs the patch preparation
automatically. The script does not change the submodule revision; Git's normal
gitlink selects the dependency. The temporary source patch only extends
exhaustive Phase matches with EIP-8321. A long-term upstream dependency update
can remove that patch once the dependency itself supports the new phase.
