#!/usr/bin/env bash
# End-to-end test of the read-only Mail feature against a real IMAP server.
#
# Starts a throwaway Dovecot (TLS on 1993 with a self-signed certificate,
# STARTTLS on 1143) with one mailbox holding two unread messages, a private
# session bus with an unlocked GNOME Keyring, and Perch under Xvfb. Then:
#   - stores the password with `perch mail-password` (CLI -> Secret Service)
#   - Perch signs in over TLS and over STARTTLS and lists the unread mail,
#     with RFC 2047 subjects decoded
#   - a newly delivered message is pushed (IMAP IDLE) within seconds
#   - nothing was marked as read on the server (read-only guarantee)
#   - a wrong password shows "sign-in failed" and is not retried
#   - removing the feature stops the sessions
# Writes mail-test.txt (and a cropped screenshot of the Mail tab) to --out.
#
# Needs root (creates a system user for the mailbox) and a private session bus
# for the keyring. Runs in the CI image:
#   dbus-run-session -- scripts/mail-test.sh [--out DIR] [--keep]   (--keep: leave Dovecot running)
set -euo pipefail

OUT=artifacts/linux
KEEP=0
SERVE=0
while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT=$2; shift 2 ;;
        --keep) KEEP=1; shift ;;
        --serve) SERVE=1; shift ;;  # only run Dovecot (on all interfaces) for a test from another machine
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
BIN=${PERCH_BIN:-target/release/perch}
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")
RESULTS=$OUT/mail-test.txt
: >"$RESULTS"
FAILS=0
check() { if [ "$2" = 1 ]; then echo "[PASS] $1: $3" | tee -a "$RESULTS"; else echo "[FAIL] $1: $3" | tee -a "$RESULTS"; FAILS=$((FAILS + 1)); fi; }
info() { echo "[INFO] $*" | tee -a "$RESULTS"; }

M=$(mktemp -d /tmp/perch-mail.XXXX)
chmod 755 "$M"
PIDS=()
cleanup() {
    for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null || true; done
    [ "$KEEP" = 1 ] || { doveadm -c "$M/dovecot.conf" stop 2>/dev/null || true; }
    rm -rf "$M"
}
trap cleanup EXIT

# --- Dovecot ------------------------------------------------------------------
id mailtest >/dev/null 2>&1 || useradd -m -s /usr/sbin/nologin mailtest
MAILHOME=/home/mailtest
rm -rf "$MAILHOME/Maildir"
mkdir -p "$MAILHOME/Maildir"/{new,cur,tmp}
chown -R mailtest:mailtest "$MAILHOME/Maildir"
# A private CA and a server certificate it signed, as on a self-hosted server.
# (A bare self-signed certificate is a CA certificate, which TLS libraries
# rightly refuse as a server certificate.)
openssl req -x509 -newkey rsa:2048 -nodes -days 2 -subj "/CN=Perch Test CA" \
    -keyout "$M/ca.key" -out "$M/ca.pem" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=localhost" \
    -keyout "$M/key.pem" -out "$M/srv.csr" 2>/dev/null
printf 'subjectAltName=DNS:localhost\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n' >"$M/ext.cnf"
openssl x509 -req -in "$M/srv.csr" -CA "$M/ca.pem" -CAkey "$M/ca.key" -CAcreateserial \
    -days 2 -extfile "$M/ext.cnf" -out "$M/cert.pem" 2>/dev/null
chmod 644 "$M/cert.pem" "$M/ca.pem"
echo "test@example.com:{PLAIN}correct horse:$(id -u mailtest):$(id -g mailtest)::$MAILHOME" >"$M/passwd"
cat >"$M/dovecot.conf" <<CONF
protocols = imap
listen = $( [ "$SERVE" = 1 ] && echo "*" || echo 127.0.0.1 )
base_dir = $M/run
log_path = $M/dovecot.log
ssl = yes
ssl_cert = <$M/cert.pem
ssl_key = <$M/key.pem
disable_plaintext_auth = yes
auth_mechanisms = plain login
passdb {
  driver = passwd-file
  args = scheme=PLAIN $M/passwd
}
userdb {
  driver = passwd-file
  args = $M/passwd
}
mail_location = maildir:~/Maildir
service imap-login {
  inet_listener imap {
    port = 1143
  }
  inet_listener imaps {
    port = 1993
    ssl = yes
  }
}
CONF
dovecot -c "$M/dovecot.conf"
for _ in $(seq 50); do (exec 3<>/dev/tcp/127.0.0.1/1993) 2>/dev/null && break; sleep 0.1; done

deliver() { # uid-ish name, from, subject
    local f="$MAILHOME/Maildir/new/$(date +%s%N).$1.perchtest"
    printf 'From: %s\r\nTo: test@example.com\r\nSubject: %s\r\nDate: %s\r\nMessage-ID: <%s@perch.test>\r\n\r\nBody %s\r\n' \
        "$2" "$3" "$(date -R)" "$1" "$1" >"$f"
    chown mailtest:mailtest "$f"
}
deliver 1 '"Ann Lee" <ann@example.com>' 'Quarterly report'
deliver 2 '=?utf-8?Q?J=C3=BCrgen?= <jurgen@example.com>' '=?UTF-8?B?SGFsbyDwn5iA?='

if [ "$SERVE" = 1 ]; then
    cp "$M/ca.pem" "$OUT/mail-ca.pem"
    echo "serving IMAP on 1993 (TLS) and 1143 (STARTTLS); CA at $OUT/mail-ca.pem"
    while :; do sleep 3600; done
fi

# --- Perch ----------------------------------------------------------------------
N=97
while [ -e "/tmp/.X$N-lock" ] || [ -e "/tmp/.X11-unix/X$N" ]; do N=$((N + 1)); done
export DISPLAY=:$N
Xvfb "$DISPLAY" -screen 0 1920x1080x24 -nolisten tcp >/dev/null 2>&1 &
PIDS+=($!)
for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
openbox >/dev/null 2>&1 &
PIDS+=($!)

export HOME=$M/home XDG_CONFIG_HOME=$M/config XDG_DATA_HOME=$M/data XDG_CACHE_HOME=$M/cache XDG_RUNTIME_DIR=$M/xdg
mkdir -p "$HOME" "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"
export PERCH_STATUS_FILE=$M/status.json PERCH_LOG=stderr
STATUS() { jq -r "$1" "$PERCH_STATUS_FILE" 2>/dev/null || echo ""; }
until_status() { # jq-filter expected seconds
    for _ in $(seq $(($3 * 10))); do [ "$(STATUS "$1")" = "$2" ] && return 0; sleep 0.1; done
    return 1
}
write_config() { # features.mail value
    cat >"$M/config.toml" <<CFG
[features]
mail = $1
[mail]
notify = true
[[mail.accounts]]
name = "TLS"
username = "test@example.com"
host = "localhost"
port = 1993
security = "tls"
ca_file = "$M/ca.pem"
[[mail.accounts]]
name = "STARTTLS"
username = "test@example.com"
host = "localhost"
port = 1143
security = "starttls"
ca_file = "$M/ca.pem"
CFG
}
write_config true

# The keyring: a throwaway GNOME Keyring on this private session bus.
echo -n "keyring-pass" | gnome-keyring-daemon --unlock --components=secrets >/dev/null 2>&1
sleep 0.5

# Store the password through the CLI, before Perch runs (both accounts share a key).
echo "correct horse" | "$BIN" --config "$M/config.toml" mail-password TLS >>"$RESULTS" 2>&1
STORED=$(secret-tool lookup application perch perch-key "mail:test@example.com@localhost" 2>/dev/null || true)
check password_in_keyring "$([ "$STORED" = "correct horse" ] && echo 1 || echo 0)" "secret-tool sees the item written by perch mail-password"
check password_not_in_config "$(grep -q "correct horse" "$M/config.toml" && echo 0 || echo 1)" "config.toml has no password"

"$BIN" --config "$M/config.toml" >"$M/perch.log" 2>&1 &
PERCH=$!
PIDS+=("$PERCH")
until_status '.mail_unread' 4 15 || true
check signs_in_over_tls "$(STATUS '.mail_accounts[]' | grep -q '^TLS: online$' && echo 1 || echo 0)" "$(STATUS '.mail_accounts | join(", ")')"
check signs_in_over_starttls "$(STATUS '.mail_accounts[]' | grep -q '^STARTTLS: online$' && echo 1 || echo 0)" "$(STATUS '.mail_accounts | join(", ")')"
check unread_listed "$([ "$(STATUS .mail_unread)" = 4 ] && echo 1 || echo 0)" "mail_unread=$(STATUS .mail_unread) (2 per account, both accounts read the same mailbox)"
check subjects_decoded "$(STATUS .mail_latest | grep -q 'Jürgen - Halo 😀' && echo 1 || echo 0)" "latest: $(STATUS .mail_latest)"

import -window root -crop 560x80+680+0 "$OUT/mail-pill.png" 2>/dev/null || true
"$BIN" tab mail
xdotool mousemove 960 0
sleep 0.8
import -window root -crop 560x220+680+0 "$OUT/mail-tab.png" 2>/dev/null || true
xdotool mousemove 40 900
sleep 0.5

# A new message must be pushed (IDLE), not waited for.
T0=$(date +%s%N)
deliver 3 'Build Bot <ci@example.com>' 'Nightly build passed'
if until_status '.mail_unread' 6 10; then
    check new_mail_pushed 1 "visible after $(( ($(date +%s%N) - T0) / 1000000 )) ms: $(STATUS .mail_latest)"
else
    check new_mail_pushed 0 "mail_unread=$(STATUS .mail_unread) after 10 s"
fi

# Read-only: Dovecot must still consider every message unseen.
SEEN=$( (doveadm -c "$M/dovecot.conf" search -u test@example.com mailbox INBOX SEEN 2>>"$M/doveadm.err" || true) | wc -l)
TOTAL=$( (doveadm -c "$M/dovecot.conf" search -u test@example.com mailbox INBOX ALL 2>>"$M/doveadm.err" || true) | wc -l)
[ -s "$M/doveadm.err" ] && info "doveadm: $(head -c 300 "$M/doveadm.err")"
check nothing_marked_read "$([ "$SEEN" = 0 ] && [ "$TOTAL" = 3 ] && echo 1 || echo 0)" "$SEEN of $TOTAL messages are \\Seen on the server"

# Wrong password: reported, not retried.
echo "wrong" | "$BIN" --config "$M/config.toml" mail-password TLS >/dev/null 2>&1
for _ in $(seq 150); do STATUS '.mail_accounts[0]' | grep -q 'sign-in failed' && break; sleep 0.1; done
check wrong_password_reported "$(STATUS '.mail_accounts[0]' | grep -q 'sign-in failed' && echo 1 || echo 0)" "$(STATUS '.mail_accounts[0]')"
# Each account must try the wrong password exactly once (the first backoff
# would be 5 s, so 8 s more would show a retry).
sleep 1
FAILS_BEFORE=$(grep -c 'sign-in rejected' "$M/perch.log" 2>/dev/null || true)
sleep 8
FAILS_AFTER=$(grep -c 'sign-in rejected' "$M/perch.log" 2>/dev/null || true)
check wrong_password_not_retried "$([ "$FAILS_BEFORE" = 2 ] && [ "$FAILS_AFTER" = 2 ] && echo 1 || echo 0)" "sign-in attempts with the wrong password: $FAILS_BEFORE (one per account), 8 s later $FAILS_AFTER"

# Turning the feature off closes the sessions.
write_config false
sleep 3
# Established TCP connections to the IMAP ports (1993 = 0x07C9, 1143 = 0x0477).
OPEN=$(awk 'NR > 1 && $4 == "01" && ($3 ~ /:07C9$/ || $3 ~ /:0477$/)' /proc/net/tcp /proc/net/tcp6 2>/dev/null | wc -l)
check feature_off_disconnects "$([ "$OPEN" = 0 ] && echo 1 || echo 0)" "$OPEN IMAP connections left"

# The add-account dialog, typed into the way a person would.
cat >"$M/config.toml" <<CFG
[features]
mail = true
[mail]
notify = false
CFG
sleep 2
"$BIN" mail-add
DLG=""
for _ in $(seq 50); do
    DLG=$(xdotool search --name '^Add mail account$' 2>/dev/null | head -1 || true)
    [ -n "$DLG" ] && break
    sleep 0.1
done
check dialog_opens "$([ -n "$DLG" ] && echo 1 || echo 0)" "window ${DLG:-not found}"
if [ -n "$DLG" ]; then
    xdotool windowactivate --sync "$DLG" 2>/dev/null || true
    sleep 0.3
    import -window "$DLG" "$OUT/mail-dialog.png" 2>/dev/null || true
    # Focus starts in the address field; Tab walks address, password, name, server, port.
    xdotool type --delay 15 "test@example.com"
    xdotool key Tab
    xdotool type --delay 15 "correct horse"
    xdotool key Tab
    xdotool type --delay 15 "Dialog"
    xdotool key Tab
    xdotool key --repeat 30 BackSpace
    xdotool type --delay 15 "localhost"
    xdotool key Tab
    xdotool key --repeat 6 BackSpace
    xdotool type --delay 15 "1993"
    import -window "$DLG" "$OUT/mail-dialog-filled.png" 2>/dev/null || true
    xdotool key Return
    for _ in $(seq 50); do grep -q 'name = "Dialog"' "$M/config.toml" && break; sleep 0.1; done
    ACCT=$(tr '\n' ' ' <"$M/config.toml" | grep -o '\[\[mail.accounts\]\].*' | head -c 220)
    check dialog_adds_account "$(grep -q 'name = "Dialog"' "$M/config.toml" && grep -q 'host = "localhost"' "$M/config.toml" && grep -q 'port = 1993' "$M/config.toml" && echo 1 || echo 0)" "$ACCT"
    STORED=$(secret-tool lookup application perch perch-key "mail:test@example.com@localhost" 2>/dev/null || true)
    check dialog_password_in_keyring "$([ "$STORED" = "correct horse" ] && echo 1 || echo 0)" "the typed password is in the keyring"
    check dialog_password_not_in_config "$(grep -q 'correct horse' "$M/config.toml" && echo 0 || echo 1)" "config.toml has no password"
    check dialog_closed "$(xdotool search --name '^Add mail account$' >/dev/null 2>&1 && echo 0 || echo 1)" "the dialog closed after Add"
    # The test server uses a private CA: point the new account at it (the
    # reload restarts that account's session).
    sed -i "s|^ca_file = \"\"|ca_file = \"$M/ca.pem\"|" "$M/config.toml"
    for _ in $(seq 150); do STATUS '.mail_accounts[]' | grep -q '^Dialog: online$' && break; sleep 0.1; done
    check dialog_account_online "$(STATUS '.mail_accounts[]' | grep -q '^Dialog: online$' && echo 1 || echo 0)" "$(STATUS '.mail_accounts | join(", ")')"
fi
RSS=$(awk '/VmRSS/ {print $2}' /proc/"$PERCH"/status)
info "Perch VmRSS after the run: $((RSS / 1024)) MB"

"$BIN" quit 2>/dev/null || true
echo "--- perch log (mail lines) ---" >>"$RESULTS"
grep -i "mail" "$M/perch.log" >>"$RESULTS" 2>/dev/null || true
if [ "$FAILS" -gt 0 ]; then
    echo "--- dovecot log ---" >>"$RESULTS"
    tail -n 40 "$M/dovecot.log" >>"$RESULTS" 2>/dev/null || true
    echo "mail test FAILED ($FAILS)"
    exit 1
fi
echo "mail test passed"
