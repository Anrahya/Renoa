"""renoa-deploy against a scratch root: real files, archives and SQLite, with
systemd, the Host inspection, HTTP and sleeping replaced."""

import hashlib
import importlib.machinery
import importlib.util
import io
import json
import sqlite3
import tarfile
import tempfile
import unittest
from contextlib import closing, redirect_stderr, redirect_stdout
from pathlib import Path

SCRIPT = Path(__file__).resolve().parents[1] / "renoa-deploy"
_loader = importlib.machinery.SourceFileLoader("renoa_deploy", str(SCRIPT))
_spec = importlib.util.spec_from_loader("renoa_deploy", _loader)
deploy = importlib.util.module_from_spec(_spec)
_loader.exec_module(deploy)

SERVICES = json.loads((SCRIPT.parent / "release.json").read_text())["services"]
UNIT = "[Service]\nExecStart=/usr/local/bin/{name}\n"


class FakeHost(deploy.Host):
    def __init__(self, root: Path):
        super().__init__(root)
        self.commands: list[tuple[str, ...]] = []
        self.turns = 0
        self.restart_during_settle: set[str] = set()
        self._settled = False

    def run(self, *args):
        self.commands.append(args)
        stdout = ""
        if args[:2] == ("systemctl", "is-active"):
            stdout = "active\n"
        elif args[:2] == ("systemctl", "show"):
            unit = args[2]
            moved = self._settled and unit in self.restart_during_settle
            stdout = f"ActiveEnterTimestampMonotonic={'2' if moved else '1'}\nActiveState=active\n"
        elif args[0] == "runuser":
            active = {"id": "op"} if self.turns else None
            stdout = json.dumps({"sessions": [{"active_operation": active}]})
        return deploy.subprocess.CompletedProcess(args, 0, stdout, "")

    def http_status(self, url, method="GET", headers=None):
        return 403 if method == "POST" else 401

    def sleep(self, seconds):
        self._settled = True

    def restarted(self) -> list[str]:
        return [command[2] for command in self.commands if command[:2] == ("systemctl", "try-restart")]


def sha(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def release_archive(directory: Path, tag: str, binaries: dict[str, bytes], assets=("app-1.js",), adapter=b"v1", corrupt=None) -> Path:
    archive = directory / f"renoa-{tag}.tar.gz"
    files = {"release.json": None}
    binaries = {"renoa-coordinator": b"renoa-coordinator-v0", "renoa-host": b"renoa-host-v0", **binaries}
    for name, data in binaries.items():
        files[f"bin/{name}"] = data
    for service in SERVICES:
        for unit in [service["unit"], *service.get("companion_units", [])]:
            files[f"units/{unit}"] = UNIT.format(name=unit).encode()
    files["adapters/model-provider-node/dist/main.js"] = adapter
    files["control-room/index.html"] = f"<html>{tag}</html>".encode()
    for asset in assets:
        files[f"control-room/assets/{asset}"] = asset.encode()
    files["renoa-deploy"] = SCRIPT.read_bytes()
    manifest = {
        "tag": tag,
        "commit": f"commit-{tag}",
        "binaries": {name: sha(data) for name, data in binaries.items()},
        "services": SERVICES,
    }
    if corrupt:
        manifest["binaries"][corrupt] = "0" * 64
    files["release.json"] = json.dumps(manifest).encode()
    with tarfile.open(archive, "w:gz") as bundle:
        for relative, data in files.items():
            info = tarfile.TarInfo(f"renoa-{tag}/{relative}")
            info.size = len(data)
            info.mode = 0o755 if relative.startswith("bin/") else 0o644
            bundle.addfile(info, io.BytesIO(data))
    return archive


class DeployTest(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.root = Path(self.scratch.name) / "root"
        self.archives = Path(self.scratch.name) / "archives"
        self.archives.mkdir()
        self.host = FakeHost(self.root)
        self.layout = deploy.Layout(self.host)
        self.hand_installed()

    def tearDown(self):
        self.scratch.cleanup()

    def hand_installed(self):
        """The layout PR-era deploys left: real files, one old backup."""
        self.layout.bin.mkdir(parents=True)
        self.layout.units.mkdir(parents=True)
        for name in ("renoa-node", "renoa-host", "renoa-coordinator"):
            (self.layout.bin / name).write_bytes(f"{name}-v0".encode())
        for unit in ("renoa-node.service", "renoa-coordinator.service"):
            (self.layout.units / unit).write_text(UNIT.format(name=unit))
        (self.layout.adapters / "model-provider-node/dist").mkdir(parents=True)
        (self.layout.adapters / "model-provider-node/dist/main.js").write_bytes(b"v1")
        (self.layout.control_room / "assets").mkdir(parents=True)
        (self.layout.control_room / "index.html").write_text("<html>v0</html>")
        (self.layout.control_room / "assets/app-0.js").write_text("app-0.js")
        (self.layout.old_backup / "bin").mkdir(parents=True)
        (self.layout.home / "state").mkdir(parents=True)
        (self.layout.home / "config").mkdir()
        (self.layout.home / "config/host.json").write_text("{}")
        with closing(sqlite3.connect(self.layout.home / "state/host.sqlite3")) as database:
            database.execute("CREATE TABLE agents (id TEXT)")
            database.execute("INSERT INTO agents VALUES ('renoa')")
            database.commit()

    def install(self, archive: Path, **options) -> int:
        with redirect_stdout(io.StringIO()), redirect_stderr(io.StringIO()):
            return deploy.install(self.host, archive, **options)

    def releases(self) -> set[str]:
        return {path.name for path in self.layout.releases.iterdir()}

    def test_the_first_install_adopts_the_hand_installed_layout_and_restarts_only_what_changed(self):
        archive = release_archive(
            self.archives,
            "v1",
            {"renoa-node": b"renoa-node-v1"},
        )

        self.assertEqual(self.install(archive), 0)

        self.assertEqual(self.host.restarted(), ["renoa-node.service"])
        self.assertEqual(self.layout.current.resolve().name, "v1")
        self.assertEqual(self.layout.previous.resolve().name, "legacy")
        self.assertTrue((self.layout.bin / "renoa-node").is_symlink())
        self.assertEqual((self.layout.bin / "renoa-node").read_bytes(), b"renoa-node-v1")
        self.assertEqual((self.layout.releases / "legacy/bin/renoa-node").read_bytes(), b"renoa-node-v0")
        self.assertTrue(self.layout.adapters.is_symlink())
        self.assertEqual((self.layout.control_room / "index.html").read_text(), "<html>v1</html>")
        with closing(sqlite3.connect(self.layout.releases / "legacy/snapshot/state/host.sqlite3")) as backup:
            self.assertEqual(backup.execute("SELECT id FROM agents").fetchall(), [("renoa",)])
        self.assertTrue((self.layout.releases / "legacy/snapshot/config/host.json").is_file())
        self.assertFalse(self.layout.old_backup.exists(), "the hand-made backup is replaced")
        self.assertEqual(self.releases(), {"v1", "legacy"})
        self.assertEqual(json.loads(self.layout.manifest.read_text())["health"], "ok")
        self.assertTrue(self.layout.tool.is_file())
        self.assertIn(("runuser", "-u", "renoa-arcee", "--", "/usr/local/bin/renoa-host", "inspect", deploy.HOME), self.host.commands)

    def test_reinstalling_the_running_release_changes_nothing(self):
        archive = release_archive(self.archives, "v1", {"renoa-node": b"renoa-node-v1"})
        self.install(archive)
        self.host.commands.clear()

        self.assertEqual(self.install(archive), 0)

        self.assertEqual(self.host.commands, [])

    def test_a_binary_that_does_not_match_its_manifest_is_refused_before_anything_changes(self):
        self.install(release_archive(self.archives, "v1", {"renoa-node": b"renoa-node-v1"}))
        self.host.commands.clear()
        bad = release_archive(self.archives, "v2", {"renoa-node": b"renoa-node-v2"}, corrupt="renoa-node")

        with self.assertRaises(deploy.DeployError):
            self.install(bad)

        self.assertEqual(self.releases(), {"v1", "legacy"})
        self.assertEqual(self.layout.current.resolve().name, "v1")
        self.assertEqual(self.host.commands, [])

    def test_running_turns_stop_the_switch_and_leave_no_staged_release(self):
        self.install(release_archive(self.archives, "v1", {"renoa-node": b"renoa-node-v1"}))
        self.host.turns = 1

        with self.assertRaises(deploy.DeployError):
            self.install(release_archive(self.archives, "v2", {"renoa-node": b"renoa-node-v2"}), drain_timeout=20)

        self.assertEqual(self.releases(), {"v1", "legacy"})
        self.assertEqual(self.layout.current.resolve().name, "v1")
        self.assertEqual(self.host.restarted(), ["renoa-node.service"], "only v1's restart happened")

    def test_a_failed_health_check_keeps_the_new_release_running_and_every_backup(self):
        self.install(release_archive(self.archives, "v1", {"renoa-node": b"renoa-node-v1"}))
        self.host.restart_during_settle = {"renoa-node.service"}
        self.host._settled = False

        code = self.install(release_archive(self.archives, "v2", {"renoa-node": b"renoa-node-v2"}))

        self.assertEqual(code, 1)
        self.assertEqual(self.layout.current.resolve().name, "v2")
        self.assertEqual(self.layout.previous.resolve().name, "v1")
        self.assertEqual(self.releases(), {"v2", "v1", "legacy"}, "nothing is pruned after a failure")
        self.assertIn("restarted during the settle window", json.loads(self.layout.manifest.read_text())["health"][0])

    def test_the_previous_control_room_assets_stay_available(self):
        self.install(release_archive(self.archives, "v1", {"renoa-node": b"h1"}, assets=("app-1.js",)))
        self.install(release_archive(self.archives, "v2", {"renoa-node": b"h2"}, assets=("app-2.js",)))

        assets = {path.name for path in (self.layout.control_room / "assets").iterdir()}
        self.assertEqual(assets, {"app-1.js", "app-2.js"})

    def test_services_not_installed_on_this_host_are_left_alone(self):
        self.install(release_archive(self.archives, "v1", {"renoa-node": b"h1", "renoa-discord": b"d1"}))

        self.assertFalse((self.layout.units / "renoa-discord.service").exists())
        self.assertNotIn("renoa-discord.service", self.host.restarted())

    def test_the_node_upgrades_the_host_catalog_before_management_reads_it(self):
        (self.layout.units / "renoa-management.service").write_text(UNIT.format(name="renoa-management.service"))

        self.install(release_archive(self.archives, "v1", {"renoa-node": b"n1", "renoa-management": b"m1"}))

        self.assertEqual(self.host.restarted(), ["renoa-node.service", "renoa-management.service"])

    def test_a_changed_adapter_restarts_the_services_that_run_it(self):
        self.install(release_archive(self.archives, "v1", {"renoa-node": b"h1"}))
        self.host.commands.clear()

        self.install(release_archive(self.archives, "v2", {"renoa-node": b"h1"}, adapter=b"v2"))

        self.assertEqual(self.host.restarted(), ["renoa-node.service"])

    def test_a_release_missing_an_installed_service_binary_is_refused(self):
        self.install(release_archive(self.archives, "v1", {"renoa-node": b"h1"}))
        archive = release_archive(self.archives, "v2", {"renoa-node": b"h2"})
        manifest_less = self.archives / "v2-without-node.tar.gz"
        with tarfile.open(archive) as source, tarfile.open(manifest_less, "w:gz") as target:
            for member in source.getmembers():
                data = source.extractfile(member).read()
                if member.name.endswith("release.json"):
                    manifest = json.loads(data)
                    del manifest["binaries"]["renoa-node"]
                    data = json.dumps(manifest).encode()
                    member.size = len(data)
                target.addfile(member, io.BytesIO(data))

        with self.assertRaises(deploy.DeployError):
            self.install(manifest_less)

        self.assertEqual(self.releases(), {"v1", "legacy"})
        self.assertEqual(self.layout.current.resolve().name, "v1")

    def test_only_the_running_release_and_its_backup_are_kept(self):
        for tag in ("v1", "v2", "v3"):
            self.install(release_archive(self.archives, tag, {"renoa-node": tag.encode()}))

        self.assertEqual(self.releases(), {"v3", "v2"})
        self.assertEqual(self.layout.previous.resolve().name, "v2")


if __name__ == "__main__":
    unittest.main()
