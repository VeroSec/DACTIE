#!/bin/bash

# This function exits the script gracefully
function cleanup() {
    echo -e "\nstopping."
    exit 0
}

# This function encapsulates the logic for formatting and printing the monitoring output.
# Arguments:
#   $1: Initial PID
#   $2: Total number of open files
print_status() {
    local initial_pid="$1"
    local total_open_files="$2"
    echo "$(date '+%H:%M:%S') - Main PID ($initial_pid) - open: ${total_open_files}"
}

PROCESS_NAME="cargo"
COMMAND_ARGS="run"

echo "press ctrl+c to stop."

# Find the Process ID (PID) of the initial command.
INITIAL_PID=$(pgrep -f "$PROCESS_NAME.*$COMMAND_ARGS" | head -n 1)

if [ -z "$INITIAL_PID" ]; then
    echo "waiting for '$PROCESS_NAME $COMMAND_ARGS' to start..."
    # If the process isn't found immediately, loop and wait for it.
    sleep 0.01
    while [ -z "$INITIAL_PID" ]; do
        INITIAL_PID=$(pgrep -f "$PROCESS_NAME.*$COMMAND_ARGS" | head -n 1)
    done
fi

echo "Found '$PROCESS_NAME $COMMAND_ARGS' with PID: $INITIAL_PID"

# trap command catches the INT signal (triggered by Ctrl+C)
# and calls the cleanup function to exit gracefully.
trap cleanup INT

while true; do
    # check if the main process (INITIAL_PID) is still running.
    if ! ps -p "$INITIAL_PID" > /dev/null; then
        echo "PID $INITIAL_PID no longer running. bye!"
        break
    fi

    # `sudo lsof -p "$INITIAL_PID"` lists all open files for this specific PID.
    # `2>/dev/null` redirects stderr (errors like "process not found") to null.
    # `grep -v " txt "` filters out loaded executable code and libraries for a more relevant count.
    # `wc -l` counts the lines, effectively the number of open files.
    # `tr -d ' '` removes any leading/trailing spaces for clean arithmetic.
    OPEN_FILES_COUNT=$(sudo lsof -p "$INITIAL_PID" 2>/dev/null | grep -v " txt " | wc -l | tr -d ' ')

    # Ensure COUNT is not empty (it might be if lsof returns nothing)
    if [ -z "$OPEN_FILES_COUNT" ]; then
    	OPEN_FILES_COUNT=0
    fi

    print_status "$INITIAL_PID" "$TOTAL_OPEN_FILES"
done

