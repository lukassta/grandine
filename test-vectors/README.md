# Pinned consensus vectors

Heze and Gloas fixtures are unmodified reference outputs from consensus-specs
12c8b24bc09b12f62688d439ba20ef00b1fdfe1f. Source manifests are kept with each
case, and heze.sha256 covers their exact bytes. The FOCIL oracle is generated
from the same Python specification; see focil-oracle/SOURCE.md.

Run bash scripts/test_heze.sh. Reports contain one row per executed vector.
These commands consume the committed fixtures; they do not regenerate them.
