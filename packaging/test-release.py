import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import textwrap
import unittest


sys.dont_write_bytecode = True

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("release", ROOT / "packaging/release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
WORKFLOW = (ROOT / ".github/workflows/release.yml").read_text(encoding="utf-8")
INSTALLER = (ROOT / "packaging/windows/fastpotify.iss").read_text(encoding="utf-8")


class ReleaseHelpersTest(unittest.TestCase):
    def test_release_versions_increase_numerically_and_stay_platform_safe(self):
        self.assertEqual(release.release_version("0.8.0", 1), "0.8.1")
        self.assertEqual(release.release_version("2.4.9", 11), "2.4.20")
        self.assertEqual(release.release_version("65535.0.0", 1), "65535.0.1")
        self.assertEqual(release.release_version("0.0.65534", 1), "0.0.65535")

        for base, run_number in [
            ("0.8.0", 0),
            ("0.8.0", -1),
            ("01.8.0", 1),
            ("0.8.0-rc1", 1),
            ("65536.0.0", 1),
            ("0.65536.0", 1),
            ("0.0.65535", 1),
        ]:
            with self.subTest(base=base, run_number=run_number):
                with self.assertRaises(ValueError):
                    release.release_version(base, run_number)

    def test_publication_order_rejects_equal_and_older_versions(self):
        release.require_newer("0.8.10", "v0.8.9")
        for candidate in ["0.8.9", "0.8.8"]:
            with self.subTest(candidate=candidate):
                with self.assertRaisesRegex(ValueError, "must be newer"):
                    release.require_newer(candidate, "v0.8.9")

    def test_stamp_changes_only_the_root_package_version(self):
        manifest = """[package]
name = "fastpotify"
version = "0.8.0"

[dependencies]
example = "0.8.0"
"""
        lockfile = """version = 4

[[package]]
name = "fastpotify"
version = "0.8.0"
dependencies = ["example"]

[[package]]
name = "example"
version = "0.8.0"
"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest_path = root / "Cargo.toml"
            lock_path = root / "Cargo.lock"
            manifest_path.write_text(manifest, encoding="utf-8")
            lock_path.write_text(lockfile, encoding="utf-8")

            release.stamp_version(manifest_path, lock_path, "0.8.27")

            self.assertEqual(
                manifest_path.read_text(encoding="utf-8"),
                manifest.replace('version = "0.8.0"', 'version = "0.8.27"', 1),
            )
            self.assertEqual(
                lock_path.read_text(encoding="utf-8"),
                lockfile.replace('version = "0.8.0"', 'version = "0.8.27"', 1),
            )

    def test_malformed_lockfile_fails_before_either_source_file_changes(self):
        manifest = '[package]\nname = "fastpotify"\nversion = "0.8.0"\n\n[dependencies]\n'
        malformed_lock = '[[package]]\nname = "another-package"\nversion = "0.8.0"\n'
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            manifest_path = root / "Cargo.toml"
            lock_path = root / "Cargo.lock"
            manifest_path.write_text(manifest, encoding="utf-8")
            lock_path.write_text(malformed_lock, encoding="utf-8")

            with self.assertRaisesRegex(ValueError, "found 0"):
                release.stamp_version(manifest_path, lock_path, "0.8.1")

            self.assertEqual(manifest_path.read_text(encoding="utf-8"), manifest)
            self.assertEqual(lock_path.read_text(encoding="utf-8"), malformed_lock)

    def test_release_assets_are_exactly_the_two_nonempty_installers(self):
        expected = {"spotidark-macos.dmg", "spotidark-windows.exe"}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in expected:
                (root / name).write_bytes(b"installer")
            release.verify_assets(root, expected)

            (root / "checksums.txt").write_text("unexpected", encoding="utf-8")
            with self.assertRaisesRegex(ValueError, "must be exactly"):
                release.verify_assets(root, expected)
            (root / "checksums.txt").unlink()

            (root / "spotidark-macos.dmg").write_bytes(b"")
            with self.assertRaisesRegex(ValueError, "assets are empty"):
                release.verify_assets(root, expected)


class ReleaseWorkflowContractTest(unittest.TestCase):
    def _run_publication(self, scenario):
        step = WORKFLOW.split("- name: Verify uploaded bytes and publish current main", 1)[1]
        step = step.split("- name: Verify published tag", 1)[0]
        script = textwrap.dedent(step.split("script: |\n", 1)[1])
        harness = """
const harnessFs = require('node:fs');
const harnessCrypto = require('node:crypto');
const updates = [];
const sha = process.env.RELEASE_SHA;
const makeAsset = name => {
  const bytes = harnessFs.readFileSync(`dist/${name}`);
  return {name, state: 'uploaded', size: bytes.length,
    digest: `sha256:${harnessCrypto.createHash('sha256').update(bytes).digest('hex')}`};
};
const assets = ['spotidark-macos.dmg', 'spotidark-windows.exe'].map(makeAsset);
if (process.env.SCENARIO === 'wrong-digest') assets[0].digest = `sha256:${'0'.repeat(64)}`;
const fixture = {data: {
  id: 7, tag_name: 'v0.8.2', draft: true, target_commitish: sha, assets,
}};
const unrelated = {id: 6, tag_name: 'v0.8.1', draft: true, target_commitish: sha, assets};
const releases = process.env.SCENARIO === 'missing-draft' ? [unrelated] :
  process.env.SCENARIO === 'ambiguous-draft' ?
    [fixture.data, {...fixture.data, id: 8}, unrelated] : [fixture.data, unrelated];
const github = {rest: {repos: {
  getReleaseByTag: async () => { const error = new Error('Not Found'); error.status = 404; throw error; },
  listReleases: async () => ({data: releases}),
  getLatestRelease: async () => ({data: {tag_name: 'v0.8.1'}}),
  updateRelease: async update => updates.push(update),
}, git: {getRef: async ({ref}) => ({data: {object: {sha:
  ref === 'heads/main' && process.env.SCENARIO === 'stale-main' ? 'f'.repeat(40) : sha}}})}},
paginate: async () => releases};
const context = {repo: {owner: 'darkroomengineering', repo: 'spotidark'}};
const core = {setFailed: message => { throw new Error(message); }};
let error = null;
(async () => {
""" + script + """
})().catch(reason => { error = reason.message; }).finally(() => {
  console.log(JSON.stringify({error, updates}));
});
"""
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "dist").mkdir()
            (root / "packaging").mkdir()
            (root / "dist/spotidark-macos.dmg").write_bytes(b"signed mac installer")
            (root / "dist/spotidark-windows.exe").write_bytes(b"windows installer")
            (root / "packaging/release.py").write_text(
                (ROOT / "packaging/release.py").read_text(encoding="utf-8"), encoding="utf-8"
            )
            environment = os.environ | {
                "RELEASE_SHA": "a" * 40,
                "RELEASE_TAG": "v0.8.2",
                "RELEASE_VERSION": "0.8.2",
                "SCENARIO": scenario,
            }
            result = subprocess.run(
                ["node", "-e", harness], cwd=root, env=environment,
                capture_output=True, text=True, check=True, timeout=10
            )
            return json.loads(result.stdout)

    def test_only_verified_bytes_for_current_main_leave_draft_state(self):
        valid = self._run_publication("valid")
        self.assertIsNone(valid["error"])
        self.assertEqual(
            valid["updates"],
            [{"owner": "darkroomengineering", "repo": "spotidark", "release_id": 7,
              "draft": False, "make_latest": "true"}],
        )
        for scenario, message in [
            ("wrong-digest", "uploaded bytes do not match"),
            ("stale-main", "Main changed during upload"),
        ]:
            with self.subTest(scenario=scenario):
                rejected = self._run_publication(scenario)
                self.assertIn(message, rejected["error"])
                self.assertEqual(rejected["updates"], [])

        for scenario in ["missing-draft", "ambiguous-draft"]:
            with self.subTest(scenario=scenario):
                rejected = self._run_publication(scenario)
                self.assertEqual(
                    rejected["error"],
                    "Expected exactly one release draft for this version.",
                )
                self.assertEqual(rejected["updates"], [])

    def _run_prune(self, scenario):
        step = WORKFLOW.split(
            "- name: Verify published tag, latest release, and prune prior Spotidark versions",
            1,
        )[1]
        script = textwrap.dedent(step.split("script: |\n", 1)[1])
        harness = """
const deleted = [];
const messages = [];
const sha = process.env.RELEASE_SHA;
const current = {
  id: 20,
  tag_name: 'v0.8.10',
  name: 'Spotidark 0.8.10',
  draft: process.env.SCENARIO === 'current-draft',
  prerelease: false,
};
const releases = [
  current,
  {id: 1, tag_name: 'v0.8.9', name: 'Spotidark 0.8.9'},
  {id: 2, tag_name: 'v0.7.99', name: 'Spotidark 0.7.99'},
  {id: 3, tag_name: 'v0.8.10', name: 'Spotidark 0.8.10'},
  {id: 4, tag_name: 'v0.8.11', name: 'Spotidark 0.8.11'},
  {id: 5, tag_name: 'v0.8.8', name: 'Spotidark 0.8.8', draft: true},
  {id: 6, tag_name: 'v0.8.7', name: 'Spotidark 0.8.7', prerelease: true},
  {id: 7, tag_name: 'v0.8.6', name: 'Another app 0.8.6'},
  {id: 8, tag_name: 'v0.8.09', name: 'Spotidark 0.8.09'},
  {id: 9, tag_name: 'rolling', name: 'Spotidark rolling'},
];
const latest = () => ({data: process.env.SCENARIO === 'wrong-latest'
  ? {id: 99, tag_name: 'v0.8.11'} : current});
const github = {
  rest: {
    repos: {
      getReleaseByTag: async () => ({data: current}),
      getLatestRelease: async () => latest(),
      listReleases: async () => ({data: releases}),
      deleteRelease: async ({release_id}) => {
        if (process.env.SCENARIO === 'delete-failure') throw new Error('delete denied');
        deleted.push(release_id);
      },
    },
    git: {getRef: async () => ({data: {object: {sha:
      process.env.SCENARIO === 'wrong-tag' ? 'f'.repeat(40) : sha}}})},
  },
  paginate: async () => releases,
};
const context = {repo: {owner: 'darkroomengineering', repo: 'spotidark'}};
const core = {info: message => messages.push(message)};
const setTimeout = resolve => resolve();
let error = null;
(async () => {
""" + script + """
})().catch(reason => { error = reason.message; }).finally(() => {
  console.log(JSON.stringify({deleted, error, messages}));
});
"""
        environment = os.environ | {
            "RELEASE_SHA": "a" * 40,
            "RELEASE_TAG": "v0.8.10",
            "SCENARIO": scenario,
        }
        result = subprocess.run(
            ["node", "-e", harness],
            env=environment,
            capture_output=True,
            text=True,
            check=True,
            timeout=10,
        )
        return json.loads(result.stdout)

    def test_prune_deletes_only_older_canonical_spotidark_releases(self):
        result = self._run_prune("valid")

        self.assertIsNone(result["error"])
        self.assertEqual(result["deleted"], [1, 2])
        self.assertEqual(len(result["messages"]), 2)

    def test_prune_retains_history_until_the_current_release_is_verified(self):
        for scenario, message in [
            ("current-draft", "not public and stable"),
            ("wrong-tag", "does not identify the tested commit"),
            ("wrong-latest", "did not become /releases/latest"),
        ]:
            with self.subTest(scenario=scenario):
                result = self._run_prune(scenario)
                self.assertIn(message, result["error"])
                self.assertEqual(result["deleted"], [])

    def test_prune_surfaces_release_deletion_failures(self):
        result = self._run_prune("delete-failure")

        self.assertEqual(result["error"], "delete denied")
        self.assertEqual(result["deleted"], [])

    def test_windows_installer_has_a_spotidark_identity_and_matching_marker(self):
        marker = (ROOT / "packaging/windows/spotidark-installer.txt").read_text(
            encoding="utf-8"
        ).strip()
        self.assertIn("AppId={{6875480E-65E0-4240-AF20-181C31D7BF7C}", INSTALLER)
        self.assertIn('#define AppIdentity "Spotidark"', INSTALLER)
        self.assertIn('Source: "spotidark-installer.txt"', INSTALLER)
        self.assertEqual(marker, "spotidark-installer-v1")
        self.assertNotIn("fastpotify-installer.txt", INSTALLER)


if __name__ == "__main__":
    unittest.main()
