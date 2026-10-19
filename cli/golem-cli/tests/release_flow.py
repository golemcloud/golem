"""CLI integration scenarios for the release commands, using real Git and saved bytes."""

import base64
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile


ROOT = Path(__file__).resolve().parents[3]
SCRIPT = ROOT / "scripts/golem-release.py"
VERSION = "9.8.7-rc1"
TAG = f"v{VERSION}"
MANIFEST = "builtin-artifacts.release.json"
TARGETS = {
    "linux-amd64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "macos-amd64": "x86_64-apple-darwin",
    "macos-arm64": "aarch64-apple-darwin",
    "windows": "x86_64-pc-windows-msvc",
}

FAKE_GH = r"""
import base64
import json
import os
from pathlib import Path
import sys
from urllib.parse import unquote

root = Path(os.environ["RELEASE_FIXTURE"])
state_path = root / "state.json"
state = json.loads(state_path.read_text())
args = sys.argv[1:]
def save():
    state_path.write_text(json.dumps(state))
def respond(value):
    print(json.dumps(value))
    sys.exit(0)
def missing():
    sys.stderr.write("gh: Not Found (HTTP 404)\n")
    sys.exit(1)
if args[0] == "api":
    endpoint = unquote(args[1].split("?")[0])
    if "/releases/tags/" in endpoint:
        repo, tag = endpoint.removeprefix("repos/").split("/releases/tags/")
        key = repo + "@" + tag
        if key not in state["releases"]:
            missing()
        value = state["releases"][key]
        if value["draft"]:
            missing()
        respond({**value, "assets": [{"name": name} for name in value["files"]]})
    if endpoint.endswith("/releases"):
        repo = endpoint.removeprefix("repos/").removesuffix("/releases")
        respond([{**value, "tag_name": key.split("@", 1)[1], "assets": [{"name": name} for name in value["files"]]} for key, value in state["releases"].items() if key.startswith(repo + "@")])
    if "/actions/artifacts/" in endpoint:
        tail = endpoint.split("/actions/artifacts/")[1].split("/")
        if tail[0] not in state["artifacts"]:
            missing()
        if len(tail) > 1:
            sys.stdout.buffer.write((root / (tail[0] + ".zip")).read_bytes())
            sys.exit(0)
        respond(state["artifacts"][tail[0]])
    if "/attempts/" in endpoint:
        respond(state["attempt"])
    if endpoint.endswith("/artifacts"):
        run_id = int(endpoint.split("/runs/")[1].split("/")[0])
        respond({"artifacts": [item for item in state["artifacts"].values() if item["workflow_run"]["id"] == run_id]})
    if endpoint.endswith("/runs"):
        respond({"workflow_runs": state["runs"]})
    if "/git/ref/tags/" in endpoint:
        if state.get("tag_missing"):
            missing()
        respond({"object": {"type": "commit", "sha": state["sha"]}})
    if endpoint.endswith("/deployment-branch-policies"):
        respond({"branch_policies": state["policies"]})
    if endpoint.endswith("/variables"):
        respond({"variables": [{"name": "RELEASE_APP_ID", "value": "1234"}]})
    if endpoint.endswith("/secrets"):
        respond({"secrets": [{"name": "RELEASE_APP_PRIVATE_KEY"}]})
    if endpoint.endswith("/environments/golem-release"):
        respond({"deployment_branch_policy": {"protected_branches": False, "custom_branch_policies": True}})
if args[:2] == ["auth", "status"]:
    sys.exit(0)
if args[0] == "release":
    operation, tag = args[1:3]
    repo = args[args.index("--repo") + 1]
    key = repo + "@" + tag
    if operation == "create":
        assert key not in state["releases"]
        state["releases"][key] = {"id": len(state["releases"]) + 100, "draft": "--draft" in args, "files": {}}
        state["writes"].append(["create", key])
        save()
        sys.exit(0)
    if operation == "download":
        name = args[args.index("--pattern") + 1]
        output = Path(args[args.index("--dir") + 1]) / name
        assert not output.exists()
        output.write_bytes(base64.b64decode(state["releases"][key]["files"][name]))
        sys.exit(0)
    if operation == "upload":
        path = Path(args[3])
        assert path.name not in state["releases"][key]["files"], "Immutable asset replaced"
        state["releases"][key]["files"][path.name] = base64.b64encode(path.read_bytes()).decode()
        state["writes"].append(["upload", key, path.name])
        fail = state.pop("fail_after_upload", None)
        save()
        sys.exit(1 if fail else 0)
raise AssertionError(args)
"""

FAKE_CARGO = r"""
import json
import os
from pathlib import Path
import shutil
import sys
root = Path(os.environ["RELEASE_FIXTURE"])
state = json.loads((root / "state.json").read_text())
component = os.environ["BUILTIN_COMPONENT"]
assert os.environ["BUILTIN_DRY_RUN"] == "1" and os.environ["BUILTIN_SKIP_BUILD"] == "1"
assert sys.argv[-1] == "publish-builtin-artifact"
if component == state.get("fail_package"):
    sys.exit(1)
name = component + "-v" + os.environ["BUILTIN_VERSION"]
destination = root / "target/builtin-artifact-release" / name
if destination.exists():
    shutil.rmtree(destination)
shutil.copytree(root / "packages" / name, destination)
"""


def sha(data):
    return hashlib.sha256(data).hexdigest()


class ReleaseFlow(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        (self.root / "bin").mkdir()
        for name, source in (
            ("gh", FAKE_GH),
            ("cargo", FAKE_CARGO),
            ("wasm-tools", ""),
        ):
            path = self.root / "bin" / name
            path.write_text(f"#!{sys.executable}\n" + source)
            path.chmod(0o755)
        (self.root / ".gitignore").write_text("*\n!.gitignore\n")
        for args in (
            ["init", "-q"],
            ["config", "user.email", "test@example.invalid"],
            ["config", "user.name", "release-test"],
            ["config", "commit.gpgsign", "false"],
            ["config", "tag.gpgsign", "false"],
            ["config", "core.hooksPath", "/dev/null"],
            ["add", ".gitignore"],
            ["commit", "-qm", "release source"],
            ["tag", TAG],
        ):
            subprocess.run(
                ["git", *args], cwd=self.root, check=True, capture_output=True
            )
        self.source = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=self.root, text=True
        ).strip()
        self.env = dict(
            os.environ,
            PATH=f"{self.root / 'bin'}{os.pathsep}{os.environ['PATH']}",
            RELEASE_FIXTURE=str(self.root),
            CI="true",
            BUILTIN_VERSION=VERSION,
            GITHUB_REPOSITORY="golemcloud/golem",
            GITHUB_EVENT_NAME="workflow_dispatch",
            GITHUB_REF=f"refs/tags/{TAG}",
            GITHUB_REF_NAME=TAG,
            GITHUB_SHA=self.source,
            GITHUB_WORKFLOW_REF=f"golemcloud/golem/.github/workflows/prepare-golem-release.yaml@refs/tags/{TAG}",
            GITHUB_RUN_ID="73",
            GITHUB_RUN_ATTEMPT="1",
        )
        self.state = {
            "sha": self.source,
            "artifacts": {},
            "releases": {},
            "writes": [],
            "runs": [],
            "policies": [{"type": "tag", "name": "v*"}],
            "attempt": {
                "status": "completed",
                "conclusion": "success",
                "updated_at": "2026-01-01T00:01:00Z",
            },
        }
        self.save()
        self.inventory = json.loads((ROOT / "builtin-artifacts.json").read_text())
        (self.root / "builtin-artifacts.json").write_text(json.dumps(self.inventory))

    def save(self):
        (self.root / "state.json").write_text(json.dumps(self.state))

    def reload(self):
        self.state = json.loads((self.root / "state.json").read_text())

    def command(self, *args, ok=True):
        result = subprocess.run(
            [sys.executable, str(SCRIPT), *map(str, args)],
            cwd=self.root,
            env=self.env,
            capture_output=True,
            text=True,
        )
        self.assertEqual(result.returncode == 0, ok, result.stdout + result.stderr)
        self.reload()
        return result

    def packages(self):
        for entry in self.inventory.values():
            component = entry["component"]
            directory = self.root / "packages" / f"{component}-v{VERSION}"
            directory.mkdir(parents=True)
            data = f"unique WASM bytes for {component}".encode()
            (directory / f"{component}.wasm").write_bytes(data)
            (directory / f"{component}.wasm.sha256").write_text(
                f"{sha(data)}  {component}.wasm\n"
            )
            (directory / "provenance.json").write_text(
                json.dumps(
                    {
                        "artifact": {
                            "component": component,
                            "version": VERSION,
                            "sha256": sha(data),
                            "size": len(data),
                        },
                        "source": {"commit": self.source},
                    }
                )
            )
            (directory / f"{component}-licenses.tar.gz").write_bytes(
                f"licenses: {component}".encode()
            )
            (directory / "release-notes.md").write_text(f"Source {self.source}")
            if component in ("git-tool", "javascript-tools", "typescript-tools"):
                (directory / f"{component}.spdx.json").write_text(
                    '{"spdxVersion":"SPDX-2.3"}'
                )
            if component == "python":
                (directory / "python-source.tar.gz").write_bytes(
                    b"corresponding Python sources"
                )
                (directory / "python-source.tar.gz.sha256").write_text(
                    f"{sha(b'corresponding Python sources')}  python-source.tar.gz\n"
                )
            self.command("seal-component", "--directory", directory)

    def artifact(self, directory, name):
        artifact_id = len(self.state["artifacts"]) + 1
        archive = self.root / f"{artifact_id}.zip"
        with zipfile.ZipFile(archive, "w") as zipped:
            for path in directory.rglob("*"):
                if path.is_file():
                    zipped.write(path, path.relative_to(directory).as_posix())
        self.state["artifacts"][str(artifact_id)] = {
            "id": artifact_id,
            "name": name,
            "digest": f"sha256:{sha(archive.read_bytes())}",
            "expired": False,
            "created_at": "2026-01-01T00:00:30Z",
            "workflow_run": {"id": 73, "head_sha": self.source},
        }
        self.save()
        return str(artifact_id)

    def inputs(self):
        self.packages()
        bundle = self.root / "bundle"
        self.command(
            "package-builtins", "--directory", bundle, "--target", self.root / "target"
        )
        runtime = self.root / "runtime"
        runtime.mkdir()
        shutil.copy(bundle / MANIFEST, runtime / MANIFEST)
        shutil.copy(bundle / MANIFEST, self.root / MANIFEST)
        needs = {
            "packages": {
                "outputs": {
                    "bundle-id": self.artifact(bundle, "builtins-73-1"),
                    "manifest-id": self.artifact(runtime, "runtime-manifest-73-1"),
                }
            }
        }
        for kind in ("native", "docker"):
            for platform, target in TARGETS.items():
                if kind == "docker" and not platform.startswith("linux-"):
                    continue
                output = self.root / f"{kind}-{platform}"
                output.mkdir()
                if kind == "native":
                    for project in ("golem-cli", "golem"):
                        suffix = ".exe" if platform == "windows" else ""
                        (output / f"{project}-{target}{suffix}").write_bytes(
                            f"{target}/{project} original signed bytes".encode()
                        )
                else:
                    for prefix, binaries in (
                        (
                            "",
                            (
                                "golem-registry-service",
                                "golem-shard-manager",
                                "worker-executor",
                                "golem-worker-service",
                                "golem-component-compilation-service",
                                "golem-xfs-quotad",
                            ),
                        ),
                        ("golem-", ("golem",)),
                    ):
                        with tarfile.open(
                            output / f"{prefix}{target}.tar", "w"
                        ) as archive:
                            for binary in binaries:
                                data = f"{target}/{binary}".encode()
                                info = tarfile.TarInfo(f"{target}/release/{binary}")
                                info.size = len(data)
                                archive.addfile(info, io.BytesIO(data))
                self.command(
                    "seal-output",
                    "--directory",
                    output,
                    "--kind",
                    kind,
                    "--target",
                    target,
                )
                needs[f"{kind}-{platform}"] = {
                    "outputs": {
                        "artifact-id": self.artifact(output, f"{kind}-{target}-73-1")
                    }
                }
        self.env["RELEASE_INPUTS"] = json.dumps(needs)
        self.env["GITHUB_RUN_ATTEMPT"] = "2"
        return needs

    def prepared(self):
        self.inputs()
        publication = self.root / "publication"
        self.command("collect", "--directory", publication)
        self.command("publish", "--directory", publication)
        ready = self.root / "ready"
        ready.mkdir()
        shutil.copy(publication / "release-ready.json", ready / "release-ready.json")
        self.artifact(ready, "release-ready-73-2")
        self.state["runs"] = [
            {
                "id": 73,
                "run_attempt": 2,
                "repository": {"full_name": "golemcloud/golem"},
                "path": ".github/workflows/prepare-golem-release.yaml",
                "head_sha": self.source,
                "head_branch": TAG,
            }
        ]
        self.save()

    def test_last_packaging_failure_has_no_public_writes(self):
        self.packages()
        self.state["fail_package"] = "web-fetch"
        self.save()
        self.command(
            "package-builtins",
            "--directory",
            self.root / "bundle",
            "--target",
            self.root / "target",
            ok=False,
        )
        self.assertEqual(self.state["writes"], [])
        self.assertFalse((self.root / "bundle/bundle.json").exists())

    def test_component_packaging_preserves_the_existing_asset_contract(self):
        shutil.copy(ROOT / "Makefile.toml", self.root / "Makefile.toml")
        (self.root / "scripts").mkdir()
        shutil.copy(SCRIPT, self.root / "scripts/golem-release.py")
        (self.root / "Cargo.toml").write_text("[workspace]\nmembers = []\n")
        paths = [
            "LICENSE",
            "builtin-tools/bash/LICENSE",
            "builtin-tools/bash/NOTICE",
            "builtin-tools/web-fetch/licenses/THIRD_PARTY_LICENSES.html",
            "builtin-tools/js-ts-tools/licenses/SOURCE.md",
            "builtin-tools/git/licenses/SOURCE.md",
            "builtin-tools/python/licenses/LICENSE",
            "builtin-tools/python/SOURCE.md",
        ]
        for component, path in (
            ("javascript-tools", "builtin-tools/js-ts-tools/licenses/npm"),
            ("typescript-tools", "builtin-tools/js-ts-tools/licenses/typescript"),
            ("git-tool", "builtin-tools/git/licenses/git"),
        ):
            paths.extend([f"{path}/LICENSE", f"{path}/sbom.spdx.json"])
        for name in paths:
            path = self.root / name
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(f"Source input {name}")
        # Stand in for the expensive vendoring boundary, not the Makefile's packaging.
        (self.root / "builtin-tools/python/package_source.py").write_text(
            "import hashlib, pathlib, sys\np = pathlib.Path(sys.argv[1])\ndata = b'locked Python source fixture'\n(p / 'python-source.tar.gz').write_bytes(data)\n(p / 'python-source.tar.gz.sha256').write_text(hashlib.sha256(data).hexdigest() + '  python-source.tar.gz\\n')\n"
        )
        subprocess.run(
            ["git", "add", "--force", *paths],
            cwd=self.root,
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["git", "commit", "-qm", "component license sources"],
            cwd=self.root,
            check=True,
            capture_output=True,
        )
        self.source = subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=self.root, text=True
        ).strip()
        self.env.update(
            GITHUB_SHA=self.source,
            CARGO_TARGET_DIR=str(self.root / "target"),
            BUILTIN_DRY_RUN="1",
            BUILTIN_SKIP_BUILD="1",
        )
        (self.root / "bin/cargo").unlink()
        for entry in self.inventory.values():
            component = entry["component"]
            wasm = self.root / entry["path"]
            wasm.parent.mkdir(parents=True, exist_ok=True)
            wasm.write_bytes(f"built {component}".encode())
            result = subprocess.run(
                [
                    "cargo",
                    "make",
                    "--disable-check-for-updates",
                    "--no-workspace",
                    "--profile",
                    "ci",
                    "publish-builtin-artifact",
                ],
                cwd=self.root,
                env={**self.env, "BUILTIN_COMPONENT": component},
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            package = (
                self.root / f"target/builtin-artifact-release/{component}-v{VERSION}"
            )
            descriptor = json.loads((package / "package.json").read_text())
            self.assertEqual(descriptor["component"], component)
            self.assertEqual(
                (package / f"{component}.wasm").read_bytes(), wasm.read_bytes()
            )
            for name, expected in descriptor["files"].items():
                self.assertEqual(sha((package / name).read_bytes()), expected)
            if component in ("git-tool", "javascript-tools", "typescript-tools"):
                self.assertIn(f"{component}.spdx.json", descriptor["files"])
            if component == "python":
                self.assertIn("python-source.tar.gz", descriptor["files"])
                self.assertIn(
                    "LGPL-3.0-only", (package / "release-notes.md").read_text()
                )
        self.reload()
        self.assertEqual(self.state["writes"], [])

    def test_fast_start_checks_tag_and_environment_without_writes(self):
        self.state["tag_missing"] = True
        self.save()
        self.command("check-start")
        for policy in (
            {"type": "branch", "name": "*"},
            {"type": "tag", "name": "some-other-tag"},
        ):
            self.state["policies"] = [policy]
            self.save()
            self.command("check-start", ok=False)
        self.assertEqual(self.state["writes"], [])

    def test_notarization_must_be_accepted_and_cleans_credentials(self):
        source = "import json, os, sys\nif sys.argv[0].endswith('xcrun'):\n    print(json.dumps({'id':'ticket', 'status':os.environ['NOTARY_STATUS']}))\n"
        for name in ("security", "codesign", "xcrun"):
            path = self.root / "bin" / name
            path.write_text(f"#!{sys.executable}\n" + source)
            path.chmod(0o755)
        (self.root / "release-native").mkdir()
        target = "aarch64-apple-darwin"
        for project in ("golem-cli", "golem"):
            (self.root / f"release-native/{project}-{target}").write_text(project)
        temporary = self.root / "signing"
        temporary.mkdir()
        env = {
            **self.env,
            "RUNNER_TEMP": str(temporary),
            "RELEASE_TARGET": target,
            "CERTIFICATE_BASE64": "Zml4dHVyZQ==",
            "CERTIFICATE_PASSWORD": "fixture",
            "APPLE_SIGNING_IDENTITY": "fixture",
            "APPLE_ID": "fixture",
            "APPLE_TEAM_ID": "fixture",
            "APPLE_APP_PASSWORD": "fixture",
        }
        for status in ("Accepted", "Invalid", "In Progress"):
            result = subprocess.run(
                ["bash", str(ROOT / "scripts/sign-release-binaries.sh")],
                cwd=self.root,
                env={**env, "NOTARY_STATUS": status},
                capture_output=True,
                text=True,
            )
            self.assertEqual(
                result.returncode == 0,
                status == "Accepted",
                result.stdout + result.stderr,
            )
            self.assertEqual(list(temporary.iterdir()), [])

    def test_partial_upload_retry_preserves_all_bytes(self):
        self.inputs()
        output = self.root / "publication"
        self.command("collect", "--directory", output)
        self.state["fail_after_upload"] = True
        self.save()
        self.command("publish", "--directory", output, ok=False)
        self.assertFalse((output / "release-ready.json").exists())
        first = json.loads(json.dumps(self.state["releases"]))
        self.command("publish", "--directory", output)
        for key, value in first.items():
            for name, data in value["files"].items():
                self.assertEqual(self.state["releases"][key]["files"][name], data)
        before = self.state["writes"][:]
        self.command("publish", "--directory", output)
        self.assertEqual(self.state["writes"], before)
        golem = self.state["releases"][f"golemcloud/golem@{TAG}"]
        self.assertEqual(len(golem["files"]), 11)
        # The last destination conflicts, while the first destination needs repair.
        key = f"golemcloud/golem-builtins@bash-v{VERSION}"
        del self.state["releases"][key]["files"]["bash.wasm"]
        golem["files"]["golem-x86_64-pc-windows-msvc.exe"] = base64.b64encode(
            b"conflicting bytes"
        ).decode()
        self.save()
        self.command("publish", "--directory", output, ok=False)
        self.assertEqual(self.state["writes"], before)
        self.assertNotIn("bash.wasm", self.state["releases"][key]["files"])

    def test_artifact_completeness_and_integrity_before_publication(self):
        needs = self.inputs()
        original = json.loads(json.dumps(self.state))
        last = needs["docker-linux-arm64"]["outputs"]["artifact-id"]
        for change in ("missing", "expired", "source", "digest", "traversal"):
            self.state = json.loads(json.dumps(original))
            if change == "missing":
                del self.state["artifacts"][last]
            elif change == "expired":
                self.state["artifacts"][last]["expired"] = True
            elif change == "source":
                self.state["artifacts"][last]["workflow_run"]["head_sha"] = "0" * 40
            elif change == "digest":
                self.state["artifacts"][last]["digest"] = "sha256:" + "0" * 64
            else:
                with zipfile.ZipFile(self.root / f"{last}.zip", "w") as zipped:
                    zipped.writestr("../escaped", "bad")
                self.state["artifacts"][last]["digest"] = "sha256:" + sha(
                    (self.root / f"{last}.zip").read_bytes()
                )
            self.save()
            self.command("collect", "--directory", self.root / change, ok=False)
            self.assertEqual(self.state["writes"], [])
        self.assertFalse((self.root / "escaped").exists())

    def test_readiness_requires_exact_completed_preparation(self):
        self.prepared()
        self.command("readiness", "--tag", TAG, "--directory", self.root / "review")
        remote = self.state["releases"][f"golemcloud/golem@{TAG}"]
        remote.update(draft=False, published_at="2026-01-01T00:02:00Z")
        self.env.update(GITHUB_EVENT_NAME="release", RELEASE_EVENT_ID=str(remote["id"]))
        self.save()
        original = json.loads(json.dumps(self.state))
        self.command(
            "readiness",
            "--tag",
            TAG,
            "--directory",
            self.root / "promotion",
            "--published",
        )
        self.command("docker-payloads", "--directory", self.root / "promotion")
        self.assertEqual(
            (self.root / "target/aarch64-unknown-linux-gnu/release/golem").read_bytes(),
            b"aarch64-unknown-linux-gnu/golem",
        )
        for change in ("late", "branch", "failed", "ambiguous", "release", "expired"):
            self.state = json.loads(json.dumps(original))
            if change == "late":
                self.state["attempt"]["updated_at"] = "2026-01-01T00:03:00Z"
            elif change == "branch":
                self.state["runs"][0]["head_branch"] = "main"
            elif change == "failed":
                self.state["attempt"]["conclusion"] = "failure"
            elif change == "ambiguous":
                self.state["runs"] *= 2
            elif change == "release":
                self.state["releases"][f"golemcloud/golem@{TAG}"]["id"] += 1
            else:
                self.state["artifacts"]["1"]["expired"] = True
            self.save()
            self.command(
                "readiness",
                "--tag",
                TAG,
                "--directory",
                self.root / change,
                "--published",
                ok=False,
            )
            self.assertEqual(self.state["writes"], original["writes"])


if __name__ == "__main__":
    unittest.main()
