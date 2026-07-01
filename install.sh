#!/bin/sh
# sonify-health quick start.  Downloads the latest released daemon for this
# platform, lays down the Star Trek bridge preset as a writable config, and runs
# it — paste-and-go, no toolchain and no choices.  Intended to be run as
# `curl ... | sh`; everything lands in the current directory.
#
# POSIX sh only: it is piped through whatever /bin/sh the host provides, and it
# must work on both Linux (GNU coreutils) and macOS (BSD tools).
set -eu

repo=LoganBarnett/sonify-health

# Map the running platform to a release-asset label.  uname exposes no long-form
# options, so its -s/-m short flags are unavoidable.  An unsupported platform
# has no prebuilt binary, so point the reader at the from-source routes.
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) label=aarch64-darwin ;;
  Darwin-x86_64) label=x86_64-darwin ;;
  Linux-x86_64) label=x86_64-linux-gnu ;;
  Linux-aarch64) label=aarch64-linux-gnu ;;
  *)
    echo "sonify-health: no prebuilt binary for $(uname -s)-$(uname -m)." >&2
    echo "Build from source instead — see the README." >&2
    exit 1
    ;;
esac

# Resolve the most recent release tag.  grep and cut take short flags here
# because their long forms are GNU-only and this also runs on macOS's BSD tools.
tag=$(curl --fail --silent --location \
  "https://api.github.com/repos/${repo}/releases/latest" \
  | grep -m1 '"tag_name":' | cut -d'"' -f4)
if [ -z "${tag}" ]; then
  echo "sonify-health: no published release yet — build from source (see README)." >&2
  exit 1
fi

# Download and unpack the daemon (tar -xzf restores the executable bit, so no
# chmod step).  Short flags again for BSD/GNU portability.
echo "sonify-health: downloading ${tag} for ${label}..."
curl --fail --location \
  "https://github.com/${repo}/releases/download/${tag}/sonify-health-server-${tag}-${label}.tar.gz" \
  | tar -xzf -

# Lay down a working, writable config unless the directory already has one, so a
# re-run does not clobber edits.
if [ ! -e config.toml ]; then
  curl --fail --silent --location --output config.toml \
    "https://raw.githubusercontent.com/${repo}/main/examples/connectivity-and-cpu-star-trek.toml"
fi

echo "sonify-health: starting — Ctrl-C to stop."
exec ./sonify-health-server --config config.toml
