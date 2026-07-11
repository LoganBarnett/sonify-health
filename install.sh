#!/bin/sh
# sonify-health quick start.  Downloads the latest released daemon for this
# platform, lays down the Star Trek bridge preset as a writable config, and runs
# it — paste-and-go, no toolchain and no choices.  Intended to be run as
# `curl ... | sh`; everything lands in the current directory.
#
# POSIX sh only: it is piped through whatever /bin/sh the host provides, and it
# must work on Linux (GNU coreutils), macOS (BSD tools), and the sh that Git
# Bash, MSYS2, and Cygwin provide on Windows.
set -eu

repo=LoganBarnett/sonify-health

# Map the running platform to a release-asset label and archive format.  uname
# exposes no long-form options, so its -s/-m short flags are unavoidable.  The
# Unix-like targets ship a .tar.gz; the Windows targets ship a .zip whose sole
# member is the .exe, so the defaults below cover the common case and the
# Windows arms override them.  Git Bash, MSYS2, and Cygwin each provide sh and
# report a MINGW*/MSYS*/CYGWIN* kernel name, so pasting this there installs the
# native Windows binary — which itself needs no such layer to run.  An
# unsupported platform has no prebuilt binary, so point the reader at the
# from-source routes.
ext=tar.gz
server=sonify-health-server
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) label=aarch64-darwin ;;
  Darwin-x86_64) label=x86_64-darwin ;;
  Linux-x86_64) label=x86_64-linux-gnu ;;
  Linux-aarch64) label=aarch64-linux-gnu ;;
  MINGW*-x86_64 | MSYS*-x86_64 | CYGWIN*-x86_64)
    label=x86_64-windows
    ext=zip
    server=sonify-health-server.exe
    ;;
  MINGW*-aarch64 | MSYS*-aarch64 | CYGWIN*-aarch64)
    label=aarch64-windows
    ext=zip
    server=sonify-health-server.exe
    ;;
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

# Download and unpack the daemon.  The Unix-like targets stream straight through
# tar (-xzf restores the executable bit, so no chmod step); a .zip is not
# streamable, so the Windows asset lands as a file that unzip — or PowerShell's
# Expand-Archive, which every Windows 10+ install ships when Git Bash bundles no
# unzip — then expands in place.  Short tar flags again for BSD/GNU portability.
asset=sonify-health-server-${tag}-${label}.${ext}
url=https://github.com/${repo}/releases/download/${tag}/${asset}
echo "sonify-health: downloading ${tag} for ${label}..."
case "${ext}" in
  tar.gz)
    curl --fail --location "${url}" | tar -xzf -
    ;;
  zip)
    curl --fail --location --output "${asset}" "${url}"
    if command -v unzip >/dev/null 2>&1; then
      # Info-ZIP unzip has only short flags: -o overwrites without prompting and
      # -q silences the extracted-file listing.
      unzip -o -q "${asset}"
    else
      powershell -NoProfile -NonInteractive -Command \
        "Expand-Archive -LiteralPath '${asset}' -DestinationPath '.' -Force"
    fi
    # BSD/macOS rm has no --force long form, so -f is unavoidable here; it also
    # keeps cleanup quiet if the archive is somehow already gone.
    rm -f "${asset}"
    ;;
esac

# Lay down a working, writable config unless the directory already has one, so a
# re-run does not clobber edits.
if [ ! -e config.toml ]; then
  curl --fail --silent --location --output config.toml \
    "https://raw.githubusercontent.com/${repo}/main/examples/connectivity-and-cpu-star-trek.toml"
fi

echo "sonify-health: starting — Ctrl-C to stop."
exec "./${server}" --config config.toml
