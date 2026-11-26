#!/bin/sh
# isolated_crt_lock.sh COMMAND ARGS...: exec COMMAND with a private, empty
# /tmp/aws_crt_cross_process_lock, the directory aws-c-common hard-codes for
# the CRT process lock (posix/cross_process_lock.c), so a pytest-xdist worker
# and the AWS CLI commands it runs see that lock as a serial run does: held
# only by their own commands. The directory is a tmpfs that disappears with
# the process. COMMAND keeps the caller's uid and gid and has no
# capabilities, so permission tests behave as without it.
# Linux only; needs unprivileged user namespaces and util-linux >= 2.36.
set -eu
uid=$(id -u)
gid=$(id -g)
exec unshare --user --map-root-user --mount sh -euc '
  uid=$1 gid=$2; shift 2
  mkdir -p /tmp/aws_crt_cross_process_lock
  mount -t tmpfs -o mode=1777 aws-cli-blackbox-crt-lock /tmp/aws_crt_cross_process_lock
  exec unshare --user --map-user="$uid" --map-group="$gid" "$@"
' sh "$uid" "$gid" "$@"
