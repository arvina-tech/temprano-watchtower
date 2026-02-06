#!/usr/bin/env bash
set -eo pipefail

# The content of the script is largely borrowed from foundryup

OUTPUT_FILE="$PWD/tempo-watchtower"

GITHUB_RELEASE_BASE_URL="https://github.com/arvina-tech/tempo-watchtower/releases/download/%s"

tolower() {
  echo "$1" | awk '{print tolower($0)}'
}

latest_release() {
  curl -s https://api.github.com/repos/arvina-tech/tempo-watchtower/releases/latest | grep -i "tag_name" | awk -F '"' '{print $4}'
}

say() {
  printf "tempo-watchtower: %s\n" "$1"
}

warn() {
  say "warning: ${1}" >&2
}

err() {
  say "$1" >&2
  exit 1
}

check_cmd() {
  command -v "$1" &>/dev/null
}

# Downloads $1 into $2 or stdout
download() {
  if [ -n "$2" ]; then
    # output into $2
    if check_cmd curl; then
      curl -#o "$2" -L "$1"
    else
      wget --show-progress -qO "$2" "$1"
    fi
  else
    # output to stdout
    if check_cmd curl; then
      curl -#L "$1"
    else
      wget --show-progress -qO- "$1"
    fi
  fi
}

get_architecture() {
    architecture=$(tolower $(uname -m))
    if [ "${architecture}" = "x86_64" ]; then
      # Redirect stderr to /dev/null to avoid printing errors if non Rosetta.
      if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = "1" ]; then
          architecture="arm64" # Rosetta.
      else
          architecture="x86_64" # Intel.
      fi
    elif [ "${architecture}" = "arm64" ] ||[ "${architecture}" = "aarch64" ] ; then
      architecture="arm64" # Arm.
    else
      architecture="x86_64" # Amd.
    fi
    echo $architecture
}

usage() {
  echo "Usage: $0 [options]"
  echo "Options:"
  echo "  -v|--version     Version of the Temprano Watchtower binary to install"
  echo "  --arch           Architecture of the Temprano Watchtower binary to install"
  echo "  --platform       Platform of the Temprano Watchtower binary to install"
  echo "  --output         Output file to install the Temprano Watchtower binary to"
  echo "  -h|--help        Show this help message"
}

main() {
  while [[ -n $1 ]]; do
    case $1 in
      --)               shift; break;;

      -v|--version)     shift; TEMPO_WATCHTOWER_VERSION=$1;;
      --arch)           shift; TEMPO_WATCHTOWER_ARCHITECTURE=$1;;
      --platform)       shift; TEMPO_WATCHTOWER_PLATFORM=$1;;
      --output)         shift; OUTPUT_FILE=$1;;
      -h|--help)
        usage
        exit 0
        ;;
      *)
        warn "unknown option: $1"
        usage
        exit 1
    esac; shift
  done

  if [ -z "$TEMPO_WATCHTOWER_ARCHITECTURE" ]; then
    TEMPO_WATCHTOWER_ARCHITECTURE=$(get_architecture)
  fi

  if [ -z "$TEMPO_WATCHTOWER_PLATFORM" ]; then
    TEMPO_WATCHTOWER_PLATFORM=$(tolower $(uname -s))
    if [ "$TEMPO_WATCHTOWER_PLATFORM" = "darwin" ]; then
      TEMPO_WATCHTOWER_PLATFORM="macos"
    elif [ "$TEMPO_WATCHTOWER_PLATFORM" = "linux" ]; then
        TEMPO_WATCHTOWER_PLATFORM="linux-musl"
    fi
  fi

  if [ "$TEMPO_WATCHTOWER_PLATFORM" != "linux-musl" -a "$TEMPO_WATCHTOWER_PLATFORM" != "macos" ]; then
    err "Unsupported platform: $TEMPO_WATCHTOWER_PLATFORM"
  fi

  if [ "$TEMPO_WATCHTOWER_PLATFORM" = "linux-musl" -a "$TEMPO_WATCHTOWER_ARCHITECTURE" != "x86_64" ]; then
    err "Linux musl is only supported on x86_64 architecture"
  fi

  if [ "$TEMPO_WATCHTOWER_PLATFORM" = "macos" -a "$TEMPO_WATCHTOWER_ARCHITECTURE" != "arm64" ]; then
    err "macOS is only supported on arm64 architecture"
  fi

  BASE_URL="$(printf $GITHUB_RELEASE_BASE_URL $(latest_release))/tempo-watchtower-%s.tar.gz"

  TEMPO_WATCHTOWER_TARGET="$TEMPO_WATCHTOWER_PLATFORM-$TEMPO_WATCHTOWER_ARCHITECTURE"
  URL="$(printf $BASE_URL $TEMPO_WATCHTOWER_TARGET)"
  TAR_OUTPUT=/tmp/tempo-watchtower.tar.gz

  download $URL $TAR_OUTPUT

  tar -C /tmp -xzf $TAR_OUTPUT tempo-watchtower

  mv /tmp/tempo-watchtower $OUTPUT_FILE
  rm $TAR_OUTPUT
  echo "Temprano Watchtower binary has been installed to $OUTPUT_FILE"
}

main $@
