#!/usr/bin/env bash
# Build a Fedora RPM for cmux-linux.
#
# Usage: from the cmux-linux workspace root:
#   packaging/build-rpm.sh
#
# Requires: rpm-build, cargo, and the GUI build deps (see README).
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$here"

echo "==> Building release binaries"
cargo build --release --locked

echo "==> Running rpmbuild"
rpmbuild -bb packaging/cmux.spec \
    --define "_sourcedir $here" \
    --define "_topdir $here/target/rpmbuild"

echo "==> RPM(s):"
find "$here/target/rpmbuild/RPMS" -name '*.rpm' -print
