#!/usr/bin/env bash
# Consumer test: an SPM package that depends on MinigrafKit, as a user's app
# does, built and tested with xcodebuild on an iOS simulator. It runs the README
# quick start and the consumer contract (consumer-test/App/Tests).
#
#   run.sh local <dist-dir> <version>   from dist-xcframework: assembles the
#                                       package the release commits to
#                                       swift-releases, with the xcframework
#                                       by path instead of by URL
#   run.sh registry <version>           .package(url:, exact: <version>)
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO="$(cd "$HERE/.." && pwd)"
URL=https://github.com/project-minigraf/minigraf-swift

# wait_for <seconds> <command...>: run the command every 30 s until it succeeds.
wait_for() {
  local limit="$1"; shift
  local start=$SECONDS
  until "$@"; do
    if (( SECONDS - start >= limit )); then
      echo "::error::gave up after ${limit}s waiting for: $*"
      return 1
    fi
    sleep 30
  done
}

# Resolve the dist directory before leaving the caller's directory.
if [ "${1:-}" = local ]; then DIST="$(cd "$2" && pwd)"; fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
cp -R "$HERE/App" "$WORK/App"
APP="$WORK/App"

case "${1:-}" in
  local)
    VERSION="$3"
    ZIP="$DIST/MinigrafKit-v$VERSION.xcframework.zip"
    [ -f "$ZIP" ] || { echo "::error::no $ZIP"; ls "$DIST"; exit 1; }
    # The swift-releases tree: Package.swift and Sources/MinigrafKit from this
    # repo, plus the generated sources, with the binary target by path. SPM
    # names a path dependency after its directory, hence minigraf-swift.
    PKG="$WORK/minigraf-swift"
    mkdir -p "$PKG/Sources"
    cp "$REPO/Package.swift" "$PKG/"
    cp -R "$REPO/Sources/MinigrafKit" "$PKG/Sources/"
    rm -f "$PKG/Sources/MinigrafKit/.gitkeep"
    cp "$DIST"/Sources/* "$PKG/Sources/MinigrafKit/"
    (cd "$PKG" && unzip -q "$ZIP")
    perl -0pi -e 's/url: "[^"]*",\s*checksum: "[^"]*"/path: "MinigrafKit.xcframework"/' "$PKG/Package.swift"
    grep -q 'path: "MinigrafKit.xcframework"' "$PKG/Package.swift" \
      || { echo "::error::could not point Package.swift at the local xcframework"; exit 1; }
    DEPENDENCY=".package(path: \"../minigraf-swift\")"
    ;;
  registry)
    VERSION="$2"
    published() {
      git ls-remote --exit-code --tags "$URL" "refs/tags/v$VERSION" > /dev/null &&
      curl -fsIL -o /dev/null "$URL/releases/download/v$VERSION/MinigrafKit-v$VERSION.xcframework.zip"
    }
    wait_for 600 published
    DEPENDENCY=".package(url: \"$URL\", exact: \"$VERSION\")"
    ;;
  *)
    echo "usage: $0 local <dist-dir> <version> | registry <version>" >&2
    exit 2
    ;;
esac

cd "$APP"
sed "s|@DEPENDENCY@|$DEPENDENCY|" Package.swift.in > Package.swift
rm Package.swift.in

# Resolve as SPM does for a user; in registry mode, the resolved version must
# be the one under test.
swift package resolve
if [ "$1" = registry ]; then
  got=$(python3 -I -c 'import json,sys; d=json.load(open("Package.resolved")); print(next(p["state"].get("version") for p in d["pins"] if p["identity"] == "minigraf-swift"))')
  [ "$got" = "$VERSION" ] || { echo "::error::resolved minigraf-swift $got, not $VERSION"; exit 1; }
  echo "resolved minigraf-swift $got"
fi

# The newest available iPhone simulator.
UDID=$(xcrun simctl list devices available --json | python3 -I -c '
import json, sys
d = json.load(sys.stdin)["devices"]
ios = sorted((k for k in d if "iOS" in k), key=lambda k: [int(x) for x in k.rsplit("iOS-", 1)[1].split("-")])
phones = [x for k in reversed(ios) for x in d[k] if x["name"].startswith("iPhone")]
print(phones[0]["udid"])')
echo "simulator: $UDID"

xcodebuild -list
xcodebuild test -scheme Consumer -destination "id=$UDID" -skipPackagePluginValidation
