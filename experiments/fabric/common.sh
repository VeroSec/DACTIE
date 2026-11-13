# Shared by generate.sh, deploy.sh and sweep.sh.
FABRIC_VERSION=${FABRIC_VERSION:-2.5.16}
HERE=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)

load_env() { set -a; source "$1"; set +a; }

# Containers that write files into this directory run as the calling user. Rootless Docker
# already maps the container's root to that user; rootful Docker (the minervas) needs --user.
local_docker_user() {
    docker info --format '{{.SecurityOptions}}' 2>/dev/null | grep -q rootless || echo "--user $(id -u):$(id -g)"
}

# Runs a shell command on the host with ssh name $1, or here when $1 is empty.
on() {
    local host=$1
    shift
    if [ -z "$host" ]; then
        bash -c "$*" < /dev/null
    else
        ssh -n -o BatchMode=yes -o LogLevel=ERROR "$host" "$*"
    fi
}

# "ssh-name|--profile ..." for every host that runs part of the network, one line per host,
# from the *_SSH variables of build/hosts.env. Empty names mean everything on this machine.
# Read with: while IFS='|' read -r host profiles; do ...; done < <(host_profiles)
host_profiles() {
    if [ -z "$ORDERER_SSH" ]; then
        echo "|--profile local"
        return
    fi
    printf '%s m1\n%s m2\n%s m3\n%s m4\n%s m4\n' "$ORDERER_SSH" "$P0ORG1_SSH" "$P0ORG2_SSH" "$P1ORG1_SSH" "$P1ORG2_SSH" |
        sort -u | awk '{p[$1] = p[$1] " --profile " $2} END {for (h in p) print h "|" p[h]}'
}

# The directory holding network/ and build/ on the hosts (REMOTE_DIR), or this one. A relative
# REMOTE_DIR is taken from the home directory on the hosts, which is asked for once, so that no
# host path has to be written into the repository.
base_dir() {
    if [ -z "$ORDERER_SSH" ]; then echo "$HERE"; return; fi
    case $REMOTE_DIR in
        /*) echo "$REMOTE_DIR" ;;
        *) echo "$(ssh -n -o BatchMode=yes -o LogLevel=ERROR "$ORDERER_SSH" 'echo "$HOME"')/$REMOTE_DIR" ;;
    esac
}

# --user for containers that write into base_dir on the hosts: evaluated by the remote shell.
host_docker_user() { if [ -z "$ORDERER_SSH" ]; then local_docker_user; else echo '--user $(id -u):$(id -g)'; fi; }
