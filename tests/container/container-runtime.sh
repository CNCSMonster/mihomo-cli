#!/usr/bin/env bash

# Shared Docker runtime discovery for Linux and macOS. The project uses the
# Docker CLI contract; on macOS the daemon may be provided by Docker Desktop or
# by Colima (which runs Linux through Apple's Virtualization.framework).

CONTAINER_CLI="${CONTAINER_CLI:-docker}"
CONTAINER_RUNTIME_KIND="unavailable"
CONTAINER_RUNTIME_ENDPOINT=""

container_runtime() {
    "$CONTAINER_CLI" "$@"
}

container_runtime_info_works() {
    container_runtime info >/dev/null 2>&1
}

container_runtime_socket_candidates() {
    case "$(uname -s)" in
        Darwin)
            printf '%s\n' \
                "$HOME/.docker/run/docker.sock" \
                "$HOME/.docker/desktop/docker.sock" \
                "$HOME/.colima/default/docker.sock" \
                "${XDG_CONFIG_HOME:-$HOME/.config}/colima/default/docker.sock"
            ;;
        Linux)
            if [[ -n "${XDG_RUNTIME_DIR:-}" ]]; then
                printf '%s\n' "$XDG_RUNTIME_DIR/docker.sock"
            fi
            printf '%s\n' "/run/user/$(id -u)/docker.sock"
            ;;
    esac
}

container_runtime_detect() {
    CONTAINER_RUNTIME_KIND="unavailable"
    CONTAINER_RUNTIME_ENDPOINT=""

    if ! command -v "$CONTAINER_CLI" >/dev/null 2>&1; then
        return 1
    fi

    if container_runtime_info_works; then
        CONTAINER_RUNTIME_KIND="docker-context"
        CONTAINER_RUNTIME_ENDPOINT="$(container_runtime context show 2>/dev/null || echo default)"
        return 0
    fi

    # An explicit endpoint is authoritative. Do not silently replace a caller's
    # requested remote daemon with a local socket.
    if [[ -n "${DOCKER_HOST:-}" ]]; then
        return 1
    fi

    local socket
    while IFS= read -r socket; do
        [[ -S "$socket" ]] || continue
        if DOCKER_HOST="unix://$socket" container_runtime info >/dev/null 2>&1; then
            export DOCKER_HOST="unix://$socket"
            CONTAINER_RUNTIME_ENDPOINT="$DOCKER_HOST"
            case "$socket" in
                */.colima/*) CONTAINER_RUNTIME_KIND="colima" ;;
                */.docker/*) CONTAINER_RUNTIME_KIND="docker-desktop" ;;
                *) CONTAINER_RUNTIME_KIND="docker-socket" ;;
            esac
            return 0
        fi
    done < <(container_runtime_socket_candidates)

    return 1
}

container_runtime_description() {
    if [[ "$CONTAINER_RUNTIME_KIND" == "unavailable" ]]; then
        printf 'unavailable'
    else
        printf '%s (%s)' "$CONTAINER_RUNTIME_KIND" "$CONTAINER_RUNTIME_ENDPOINT"
    fi
}

container_runtime_diagnostic() {
    echo "Docker CLI cannot reach a Linux container daemon." >&2
    case "$(uname -s)" in
        Darwin)
            echo "Start Docker Desktop or Colima ('colima start'), then retry." >&2
            ;;
        Linux)
            echo "Start Docker Engine, or select a working Docker context/DOCKER_HOST." >&2
            ;;
    esac
}
