"""Mercury UI WebSocket Origin/Host check (docs/02 §4, upstream PR #566).

Starts one mercury.exe per configuration (null audio, PTT none, UI WebSocket on
127.0.0.1:10000), sends WebSocket upgrade requests with chosen Origin and Host
headers, and records the HTTP status of each. A DNS-rebinding page is modelled
by connecting to 127.0.0.1 with the attacker's name in Host and Origin, which
is what the browser sends after the name is re-pointed at loopback.

    python lab/windows/ws-origin-check.py <stock mercury.exe> <patched mercury.exe> <out-dir>

Writes <out-dir>/report.json and report.md; exits 1 if any expectation fails.
"""
import base64, json, os, socket, subprocess, sys, time

PORT = 10000
HOST_OK = f'127.0.0.1:{PORT}'

# (id, description, origin or None, host header)
CASES = [
    ('no-origin',     'Non-browser client (no Origin)',                 None,                           HOST_OK),
    ('same-ip',       'Mercury\'s own page via 127.0.0.1',              f'http://127.0.0.1:{PORT}',     HOST_OK),
    ('same-localhost','Mercury\'s own page via localhost',              f'http://localhost:{PORT}',     f'localhost:{PORT}'),
    ('evil',          'Another web page (cross-origin)',                'http://evil.example',          HOST_OK),
    ('null',          'Sandboxed page / file: (Origin null)',           'null',                         HOST_OK),
    ('rebind',        'DNS-rebinding page (attacker name on loopback)', f'http://attacker.example.com:{PORT}', f'attacker.example.com:{PORT}'),
    ('hosted-app',    'Hosted web app https://rhizomatica.github.io',   'https://rhizomatica.github.io', HOST_OK),
    ('listed-lan',    'Operator-listed LAN name',                       f'http://station.example.org:{PORT}', f'station.example.org:{PORT}'),
    ('no-origin-after','Non-browser client after the refusals',         None,                           HOST_OK),
]

# (config id, exe key, extra [main] lines, expected status per case id)
ALLOW = 101; DENY = 403
CONFIGS = [
    ('stock', 'stock', [], {c[0]: ALLOW for c in CASES}),
    ('pr566-default', 'patched', ['bind_address = 127.0.0.1'],
     {'no-origin': ALLOW, 'same-ip': ALLOW, 'same-localhost': ALLOW, 'evil': DENY, 'null': DENY,
      'rebind': DENY, 'hosted-app': ALLOW, 'listed-lan': DENY, 'no-origin-after': ALLOW}),
    ('pr566-managed', 'patched', ['bind_address = 127.0.0.1', 'ui_allowed_origins = ""'],
     {'no-origin': ALLOW, 'same-ip': ALLOW, 'same-localhost': ALLOW, 'evil': DENY, 'null': DENY,
      'rebind': DENY, 'hosted-app': DENY, 'listed-lan': DENY, 'no-origin-after': ALLOW}),
    ('pr566-listed', 'patched', ['bind_address = 127.0.0.1', f'ui_allowed_origins = "http://station.example.org:{PORT}"'],
     {'no-origin': ALLOW, 'same-ip': ALLOW, 'same-localhost': ALLOW, 'evil': DENY, 'null': DENY,
      'rebind': DENY, 'hosted-app': DENY, 'listed-lan': ALLOW, 'no-origin-after': ALLOW}),
]


def upgrade(origin, host):
    key = base64.b64encode(os.urandom(16)).decode()
    req = [f'GET /websocket HTTP/1.1', f'Host: {host}', 'Upgrade: websocket', 'Connection: Upgrade',
           f'Sec-WebSocket-Key: {key}', 'Sec-WebSocket-Version: 13']
    if origin is not None:
        req.append(f'Origin: {origin}')
    s = socket.create_connection(('127.0.0.1', PORT), timeout=5)
    try:
        s.sendall(('\r\n'.join(req) + '\r\n\r\n').encode())
        buf = b''
        while b'\r\n\r\n' not in buf:
            chunk = s.recv(4096)
            if not chunk:
                break
            buf += chunk
        head, _, rest = buf.partition(b'\r\n\r\n')
        line = head.split(b'\r\n', 1)[0].decode(errors='replace')
        status = int(line.split()[1]) if len(line.split()) > 1 else 0
        frame = False
        if status == 101:            # does the UI actually talk to us?
            s.settimeout(3)
            try:
                frame = bool(rest) or bool(s.recv(1))
            except socket.timeout:
                frame = False
        return status, line, frame
    finally:
        s.close()


def wait_port(proc):
    for _ in range(100):
        if proc.poll() is not None:
            return False
        try:
            socket.create_connection(('127.0.0.1', PORT), timeout=0.3).close()
            return True
        except OSError:
            time.sleep(0.2)
    return False


def run_config(exe, out, extra):
    os.makedirs(out, exist_ok=True)
    ini = os.path.join(out, 'mercury.ini')
    with open(ini, 'w', newline='\n') as f:
        f.write('[main]\n' + ''.join(l + '\n' for l in extra) + '[store]\nenabled = false\n')
    log = os.path.join(out, 'mercury.log')
    with open(os.path.join(out, 'stdout.log'), 'wb') as so:
        p = subprocess.Popen([exe, '-C', ini, '-x', 'null', '-p', '8300', '-b', '8100', '-G', '-U', str(PORT),
                              '-W', '-P', 'none', '-L', log], stdout=so, stderr=subprocess.STDOUT)
    try:
        if not wait_port(p):
            return None
        time.sleep(1)
        return {cid: upgrade(origin, host) for cid, _, origin, host in CASES}
    finally:
        p.terminate()
        try:
            p.wait(5)
        except subprocess.TimeoutExpired:
            p.kill()


def main():
    exes = {'stock': os.path.abspath(sys.argv[1]), 'patched': os.path.abspath(sys.argv[2])}
    out = sys.argv[3]
    report, ok = [], True
    for cfg, exe_key, extra, expect in CONFIGS:
        res = run_config(exes[exe_key], os.path.join(out, cfg), extra)
        if res is None:
            ok = False
            report.append({'config': cfg, 'error': 'mercury did not start'})
            continue
        rows = []
        for cid, desc, origin, host in CASES:
            status, line, frame = res[cid]
            passed = status == expect[cid]
            ok &= passed
            rows.append({'case': cid, 'description': desc, 'origin': origin, 'host': host, 'status': status,
                         'status_line': line, 'ui_frame_received': frame, 'expected': expect[cid], 'pass': passed})
        logp = os.path.join(out, cfg, 'mercury.log')
        refused = 0
        if os.path.exists(logp):
            refused = open(logp, encoding='utf-8', errors='replace').read().count('refused a cross-origin upgrade')
        report.append({'config': cfg, 'exe': exes[exe_key], 'ini_main': extra, 'rows': rows, 'refusals_logged': refused})
    with open(os.path.join(out, 'report.json'), 'w') as f:
        json.dump(report, f, indent=2)
    md = ['# WebSocket Origin/Host check', '', f'Result: **{"PASS" if ok else "FAIL"}**', '']
    for r in report:
        md.append(f'## {r["config"]}')
        if 'error' in r:
            md += ['', r['error'], '']
            continue
        md += ['', f'`[main]`: {", ".join(r["ini_main"]) or "(defaults)"}; refusals logged: {r["refusals_logged"]}', '',
               '| Case | Origin | Host | Status | Expected | UI frame | |', '|---|---|---|---|---|---|---|']
        for x in r['rows']:
            md.append(f'| {x["description"]} | `{x["origin"]}` | `{x["host"]}` | {x["status"]} | {x["expected"]} | '
                      f'{"yes" if x["ui_frame_received"] else "no"} | {"PASS" if x["pass"] else "FAIL"} |')
        md.append('')
    with open(os.path.join(out, 'report.md'), 'w', newline='\n') as f:
        f.write('\n'.join(md))
    print('\n'.join(md))
    sys.exit(0 if ok else 1)


if __name__ == '__main__':
    main()
