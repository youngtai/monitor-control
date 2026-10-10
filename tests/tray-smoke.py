"""Private-session test of StatusNotifier registration, menu, and Quit.
Run with dbus-run-session -- xvfb-run -a /usr/bin/python3 tests/tray-smoke.py BINARY.
The fake watcher represents a desktop tray host; no user settings are changed.
"""
import os
import subprocess
import sys
import threading
import time
import tempfile

# Never load the real user's preferences or register their shortcuts.
test_config = tempfile.TemporaryDirectory(prefix="monitor-control-test-")
os.environ["XDG_CONFIG_HOME"] = test_config.name
os.environ["GSETTINGS_BACKEND"] = "memory"
from gi.repository import Gio, GLib

connection = Gio.bus_get_sync(Gio.BusType.SESSION, None)
registered = threading.Event()
item = {}
xml = '''<node><interface name="org.kde.StatusNotifierWatcher">
<method name="RegisterStatusNotifierItem"><arg type="s" direction="in"/></method>
<method name="RegisterStatusNotifierHost"><arg type="s" direction="in"/></method>
<property name="RegisteredStatusNotifierItems" type="as" access="read"/>
<property name="IsStatusNotifierHostRegistered" type="b" access="read"/>
<property name="ProtocolVersion" type="i" access="read"/>
<signal name="StatusNotifierItemRegistered"><arg type="s"/></signal>
<signal name="StatusNotifierHostRegistered"/>
</interface></node>'''

def method(conn, sender, path, interface, name, parameters, invocation):
    if name == 'RegisterStatusNotifierItem':
        value = parameters.unpack()[0]
        item['bus'] = sender if value.startswith('/') else value
        item['path'] = value if value.startswith('/') else '/StatusNotifierItem'
        registered.set()
    invocation.return_value(None)

def prop(conn, sender, path, interface, name):
    return {'RegisteredStatusNotifierItems': GLib.Variant('as', []),
            'IsStatusNotifierHostRegistered': GLib.Variant('b', True),
            'ProtocolVersion': GLib.Variant('i', 0)}[name]

def call(bus, path, interface, method, args=None):
    return connection.call_sync(bus, path, interface, method, args, None, Gio.DBusCallFlags.NONE, 5000, None)

info=Gio.DBusNodeInfo.new_for_xml(xml)
connection.register_object('/StatusNotifierWatcher', info.interfaces[0], method, prop, None)
loop=GLib.MainLoop()
threading.Thread(target=loop.run, daemon=True).start()
process = subprocess.Popen([os.path.abspath(sys.argv[1]), '--background'])
try:
    # Bring the tray host online after the app, testing offline recovery.
    subprocess.run(['gdbus', 'wait', '--session', '--timeout', '10', 'io.github.monitorcontrol.MonitorControl'], check=True, timeout=12)
    time.sleep(0.5)
    call('org.freedesktop.DBus', '/org/freedesktop/DBus', 'org.freedesktop.DBus', 'RequestName', GLib.Variant('(su)', ('org.kde.StatusNotifierWatcher', 0)))
    assert registered.wait(5), 'Tray did not register with the new host'
    menu=call(item['bus'], item['path'], 'org.freedesktop.DBus.Properties', 'Get', GLib.Variant('(ss)', ('org.kde.StatusNotifierItem', 'Menu'))).unpack()[0]
    layout=call(item['bus'], menu, 'com.canonical.dbusmenu', 'GetLayout', GLib.Variant('(iias)', (0, -1, []))).unpack()[1]
    def flatten(node):
        node_id, props, children = node
        yield node_id, props
        for child in children:
            yield from flatten(child)
    entries = list(flatten(layout))
    labels = {props.get('label'): node_id for node_id, props in entries if 'label' in props}
    assert set(labels) == {'Open Monitor Control', 'Quit'}, labels
    assert not any(props.get('children-display') == 'submenu' for node_id, props in entries if node_id != 0), entries
    call(item['bus'], menu, 'com.canonical.dbusmenu', 'Event', GLib.Variant('(isvu)', (labels['Open Monitor Control'], 'clicked', GLib.Variant('i', 0), 0)))
    call(item['bus'], menu, 'com.canonical.dbusmenu', 'Event', GLib.Variant('(isvu)', (labels['Quit'], 'clicked', GLib.Variant('i', 0), 0)))
    assert process.wait(timeout=5) == 0
    print('PASS: late tray host registration, menu contents, Quit action')
finally:
    if process.poll() is None: process.terminate(); process.wait(timeout=4)
    loop.quit()
