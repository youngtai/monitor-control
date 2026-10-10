"""Run under a private D-Bus/Xvfb session; never changes brightness."""
import os
from pathlib import Path
import subprocess
import sys
import time
import tempfile

# Never load the real user's preferences or register their shortcuts.
test_config = tempfile.TemporaryDirectory(prefix="monitor-control-test-")
os.environ["XDG_CONFIG_HOME"] = test_config.name
os.environ["GSETTINGS_BACKEND"] = "memory"

binary = os.path.abspath(sys.argv[1])
app = 'io.github.monitorcontrol.MonitorControl'
process = subprocess.Popen([binary, '--background'])
try:
    subprocess.run(['gdbus', 'wait', '--session', '--timeout', '10', app], check=True, timeout=12)
    # Wait for discovery, then test command forwarding while the primary lives.
    time.sleep(2)
    def cpu_ticks(pid):
        fields = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()
        return int(fields[11]) + int(fields[12])
    before = cpu_ticks(process.pid)
    time.sleep(2)
    consumed = (cpu_ticks(process.pid) - before) / os.sysconf('SC_CLK_TCK')
    print(f'Idle agent CPU over 2 seconds: {consumed:.3f} seconds')
    subprocess.run([binary, '--settings'], check=True, timeout=4)
    assert process.poll() is None, 'Primary process exited during second launch'
    workers = subprocess.check_output(['pgrep', '-P', str(process.pid)], text=True).split()
    assert len(workers) == 1, f'Expected one hardware worker, found {workers}'
    subprocess.run([binary, '--background'], check=True, timeout=4)
    assert subprocess.check_output(['pgrep', '-P', str(process.pid)], text=True).split() == workers
    subprocess.run([binary, '--quit'], check=True, timeout=4)
    assert process.wait(timeout=4) == 0
    time.sleep(0.2)
    for worker in workers:
        stat = Path(f'/proc/{worker}/stat')
        assert not stat.exists() or stat.read_text().split()[2] == 'Z', 'Hardware worker survived app exit'
    print('PASS: background startup, secondary launch forwarding, one worker, clean Quit')
finally:
    if process.poll() is None:
        process.terminate()
        process.wait(timeout=4)
