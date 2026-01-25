#!/bin/bash

# Entrypoint script for Bichon Docker container
# Handles PUID/PGID environment variables for proper user permissions

set -e

# If not running as root, do nothing
if [ "$(id -u)" != "0" ]; then
    echo "Running as non-root user ($(id -u)), skipping PUID/PGID handling"
    exec "$@"
fi


# Function to create user and switch to it
switch_user() {
    local puid="$1"
    local pgid="$2"
    local USER_NAME
    local GROUP_NAME

    # group
    if getent group "$pgid" >/dev/null 2>&1; then
        GROUP_NAME=$(getent group "$pgid" | cut -d: -f1)
    else
        groupadd -g "$pgid" bichon
        GROUP_NAME=bichon
    fi

    # user
    if getent passwd "$puid" >/dev/null 2>&1; then
        USER_NAME=$(getent passwd "$puid" | cut -d: -f1)
    else
        useradd -u "$puid" -g "$GROUP_NAME" -s /bin/bash -d /data bichon
        USER_NAME=bichon
    fi

    chown_recursive() {
        local dir="$1"
        if [ ! -d "$dir" ]; then
            return 0
        fi
        if touch "$dir/.test_write" 2>/dev/null && rm "$dir/.test_write" 2>/dev/null; then
            chown -R "$puid:$pgid" "$dir" || true
        else
            echo "Warning: Cannot change ownership of $dir (read-only or restricted)"
        fi
    }

    chown_recursive /data
    chown_recursive /opt/bichon
    chown_recursive /envelope
    chown_recursive /eml

    exec runuser -u "$USER_NAME" -- "$@"
}


# Check if PUID and PGID are set
if [ -n "$PUID" ] && [ -n "$PGID" ]; then
    echo "Switching to user with PUID=$PUID, PGID=$PGID"
    switch_user "$PUID" "$PGID" "$@"
else
    echo "No PUID/PGID specified, running as root"
    exec "$@"
fi