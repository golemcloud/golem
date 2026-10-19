#!/usr/bin/env python3
"""Package immutable release inputs, publish missing assets, and check readiness."""

import argparse
import fnmatch
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
from urllib.parse import quote
import zipfile


REPOSITORY = "golemcloud/golem"
BUILTINS = "golemcloud/golem-builtins"
MANIFEST = "builtin-artifacts.release.json"
WORKFLOW = ".github/workflows/prepare-golem-release.yaml"
NATIVE = {
    "linux-amd64": "x86_64-unknown-linux-gnu",
    "linux-arm64": "aarch64-unknown-linux-gnu",
    "macos-amd64": "x86_64-apple-darwin",
    "macos-arm64": "aarch64-apple-darwin",
    "windows": "x86_64-pc-windows-msvc",
}
DOCKER = {key: NATIVE[key] for key in ("linux-amd64", "linux-arm64")}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def api(endpoint, missing=False):
    result = subprocess.run(["gh", "api", endpoint], capture_output=True, text=True)
    if missing and result.returncode and "(HTTP 404)" in result.stderr:
        return None
    require(
        result.returncode == 0, f"GitHub read failed for {endpoint}: {result.stderr}"
    )
    return json.loads(result.stdout)


def pages(endpoint, key=None):
    # GitHub's per_page upper bound is 100. Stop on the first short page.
    for page in range(1, 1001):
        response = api(
            f"{endpoint}{'&' if '?' in endpoint else '?'}per_page=100&page={page}"
        )
        values = response[key] if key else response
        yield from values
        if len(values) < 100:
            return
    raise ValueError(f"Pagination limit exceeded: {endpoint}")


def load(path):
    return json.loads(Path(path).read_text(encoding="utf-8"))


def write(path, value):
    Path(path).write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def digest(path):
    with Path(path).open("rb") as file:
        return hashlib.file_digest(file, "sha256").hexdigest()


def version():
    value = os.environ.get("BUILTIN_VERSION") or os.environ.get(
        "GITHUB_REF_NAME", ""
    ).removeprefix("v")
    require(
        re.fullmatch(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?", value),
        "Set BUILTIN_VERSION to a release version without v",
    )
    return value


def context():
    sha = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    value = {"repository": REPOSITORY, "ref": f"refs/tags/v{version()}", "sha": sha}
    if os.environ.get("CI"):
        require(
            os.environ.get("GITHUB_REPOSITORY") == REPOSITORY,
            "Release requires golemcloud/golem",
        )
        require(
            os.environ.get("GITHUB_EVENT_NAME") == "workflow_dispatch",
            "Release preparation requires workflow_dispatch",
        )
        require(
            os.environ.get("GITHUB_REF") == value["ref"],
            "Release preparation requires the exact full tag ref",
        )
        require(
            os.environ.get("GITHUB_SHA") == sha,
            "Checkout differs from the workflow source",
        )
        require(
            os.environ.get("GITHUB_WORKFLOW_REF")
            == f"{REPOSITORY}/{WORKFLOW}@{value['ref']}",
            "Unexpected release preparation workflow",
        )
        value.update(
            run_id=int(os.environ["GITHUB_RUN_ID"]),
            attempt=int(os.environ["GITHUB_RUN_ATTEMPT"]),
        )
    return value


def safe_file(directory, name):
    require(
        isinstance(name, str) and name and "\\" not in name,
        f"Invalid file name: {name}",
    )
    relative = Path(name)
    require(
        not relative.is_absolute()
        and all(part not in ("..", ".") for part in relative.parts),
        f"Unsafe path: {name}",
    )
    path = directory / relative
    require(
        path.is_file()
        and not path.is_symlink()
        and path.resolve().is_relative_to(directory.resolve()),
        f"Missing or unsafe file: {path}",
    )
    return path


def hashes(directory, names):
    return {name: digest(safe_file(directory, name)) for name in sorted(names)}


def verify_files(directory, files):
    require(
        files and hashes(directory, files) == files,
        f"Incomplete or corrupt release files in {directory}",
    )


def check_manifest(path=Path(MANIFEST)):
    manifest = load(path)
    inventory = load("builtin-artifacts.json")
    require(
        manifest["schemaVersion"] == 1 and manifest["repository"] == BUILTINS,
        "Invalid runtime manifest header",
    )
    require(
        set(manifest["artifacts"]) == set(inventory),
        "Runtime manifest must match the complete inventory",
    )
    for key, entry in manifest["artifacts"].items():
        require(
            entry["component"] == inventory[key]["component"]
            and entry["version"] == version(),
            f"Wrong runtime manifest coordinate: {key}",
        )
        require(
            re.fullmatch("[0-9a-f]{64}", entry["sha256"]),
            f"Invalid WASM checksum: {key}",
        )
    return manifest


def component_files(component, directory):
    names = {
        f"{component}.wasm",
        f"{component}.wasm.sha256",
        "provenance.json",
        f"{component}-licenses.tar.gz",
        "release-notes.md",
    }
    if component == "python":
        names.update({"python-source.tar.gz", "python-source.tar.gz.sha256"})
    names.update(path.name for path in directory.glob("*.spdx.json"))
    return names


def seal_component(directory):
    provenance = load(directory / "provenance.json")
    artifact = provenance["artifact"]
    component = artifact["component"]
    require(
        component
        in {entry["component"] for entry in load("builtin-artifacts.json").values()},
        "Unknown component",
    )
    require(
        artifact["version"] == version()
        and provenance["source"]["commit"] == context()["sha"],
        "Wrong component provenance",
    )
    wasm = safe_file(directory, f"{component}.wasm")
    require(
        digest(wasm) == artifact["sha256"] and wasm.stat().st_size == artifact["size"],
        "WASM differs from provenance",
    )
    for name in (f"{component}.wasm", "python-source.tar.gz"):
        if (directory / name).exists():
            require(
                (directory / f"{name}.sha256").read_text().strip()
                == f"{digest(directory / name)}  {name}",
                f"Incorrect checksum file: {name}",
            )
    write(
        directory / "package.json",
        {
            "component": component,
            "files": hashes(directory, component_files(component, directory)),
        },
    )


def package_builtins(directory, target):
    require(
        not directory.exists(),
        f"Preserve existing packages instead of rebuilding: {directory}",
    )
    directory.mkdir(parents=True)
    manifest = {"schemaVersion": 1, "repository": BUILTINS, "artifacts": {}}
    for key, entry in load("builtin-artifacts.json").items():
        component = entry["component"]
        env = dict(
            os.environ,
            BUILTIN_COMPONENT=component,
            BUILTIN_DRY_RUN="1",
            BUILTIN_SKIP_BUILD="1",
        )
        run(
            "cargo",
            "make",
            "--disable-check-for-updates",
            "publish-builtin-artifact",
            env=env,
        )
        package = target / "builtin-artifact-release" / f"{component}-v{version()}"
        verify_files(package, load(package / "package.json")["files"])
        shutil.copytree(package, directory / component)
        manifest["artifacts"][key] = {
            "component": component,
            "version": version(),
            "sha256": digest(package / f"{component}.wasm"),
        }
    write(directory / MANIFEST, manifest)
    check_manifest(directory / MANIFEST)
    names = [
        path.relative_to(directory).as_posix()
        for path in directory.rglob("*")
        if path.is_file()
    ]
    write(
        directory / "bundle.json",
        {"context": context(), "files": hashes(directory, names)},
    )
    print(f"Packaged the complete built-in inventory in {directory}; no release writes")


def check_bundle(directory):
    bundle = load(directory / "bundle.json")
    expected = context()
    require(
        all(
            bundle["context"].get(key) == value
            for key, value in expected.items()
            if key != "attempt"
        ),
        "Built-in bundle belongs to another source or run",
    )
    verify_files(directory, bundle["files"])
    manifest = check_manifest(directory / MANIFEST)
    expected_names = {MANIFEST}
    for key, entry in manifest["artifacts"].items():
        component = entry["component"]
        package = directory / component
        descriptor = load(package / "package.json")
        require(descriptor["component"] == component, f"Wrong component package: {key}")
        require(
            set(descriptor["files"]) == component_files(component, package),
            f"Incomplete component package: {key}",
        )
        verify_files(package, descriptor["files"])
        require(
            digest(package / f"{component}.wasm") == entry["sha256"],
            f"Package differs from runtime manifest: {key}",
        )
        expected_names.update(
            f"{component}/{name}" for name in {*descriptor["files"], "package.json"}
        )
    require(
        set(bundle["files"]) == expected_names,
        "Unexpected or incomplete built-in bundle",
    )
    return bundle


def expected_output(kind, target):
    if kind == "native":
        require(target in NATIVE.values(), "Unknown native target")
        suffix = ".exe" if "windows" in target else ""
        return {f"{project}-{target}{suffix}" for project in ("golem-cli", "golem")}
    require(kind == "docker" and target in DOCKER.values(), "Unknown Docker target")
    return {f"{target}.tar", f"golem-{target}.tar"}


def seal_output(directory, kind, target):
    check_manifest()
    names = expected_output(kind, target)
    require(
        {path.name for path in directory.iterdir()} == names,
        "Unexpected or incomplete executable output",
    )
    write(
        directory / "output.json",
        {
            "context": context(),
            "kind": kind,
            "target": target,
            "manifest_sha256": digest(MANIFEST),
            "files": hashes(directory, names),
        },
    )


def check_artifact(artifact_id, run_id=None, sha=None):
    require(str(artifact_id).isdigit(), "Missing artifact ID")
    artifact = api(f"repos/{REPOSITORY}/actions/artifacts/{artifact_id}")
    require(
        not artifact["expired"] and artifact["id"] == int(artifact_id),
        f"Missing or expired artifact {artifact_id}",
    )
    require(
        artifact["workflow_run"]["id"] == int(run_id or os.environ["GITHUB_RUN_ID"]),
        f"Artifact {artifact_id} belongs to another run",
    )
    require(
        artifact["workflow_run"]["head_sha"] == (sha or os.environ["GITHUB_SHA"]),
        f"Artifact {artifact_id} belongs to another source",
    )
    require(
        re.fullmatch(r"sha256:[0-9a-f]{64}", artifact.get("digest", "")),
        f"Artifact {artifact_id} lacks a GitHub SHA-256 digest",
    )
    return artifact


def download(artifact, directory):
    require(not directory.exists(), f"Artifact destination already exists: {directory}")
    directory.mkdir(parents=True)
    with tempfile.TemporaryFile() as archive:
        run(
            "gh",
            "api",
            f"repos/{REPOSITORY}/actions/artifacts/{artifact['id']}/zip",
            stdout=archive,
        )
        archive.seek(0)
        actual = hashlib.file_digest(archive, "sha256").hexdigest()
        require(
            artifact["digest"] == f"sha256:{actual}",
            f"Artifact archive digest mismatch: {artifact['id']}",
        )
        archive.seek(0)
        with zipfile.ZipFile(archive) as zipped:
            names = set()
            for entry in zipped.infolist():
                name = entry.filename
                path = Path(name)
                require(
                    name not in names
                    and "\\" not in name
                    and not path.is_absolute()
                    and ".." not in path.parts,
                    f"Unsafe or duplicate archive path: {name}",
                )
                mode = entry.external_attr >> 16
                require(
                    stat.S_IFMT(mode) in (0, stat.S_IFREG, stat.S_IFDIR),
                    f"Unsafe archive member: {name}",
                )
                names.add(name)
            zipped.extractall(directory)


def collect(directory):
    require(
        not directory.exists(), f"Publication directory already exists: {directory}"
    )
    directory.mkdir(parents=True)
    needs = json.loads(os.environ["RELEASE_INPUTS"])
    producers = []

    def fetch(artifact_id, name):
        artifact = check_artifact(artifact_id)
        download(artifact, directory / name)
        producers.append(artifact)
        return directory / name

    builtins = fetch(needs["packages"]["outputs"]["bundle-id"], "builtins")
    check_bundle(builtins)
    runtime = fetch(needs["packages"]["outputs"]["manifest-id"], "runtime")
    manifest_sha = digest(builtins / MANIFEST)
    require(
        digest(runtime / MANIFEST) == manifest_sha,
        "Build manifest differs from packaged manifest",
    )
    destinations = builtin_destinations(builtins, directory)
    native_files = {MANIFEST: f"builtins/{MANIFEST}"}
    for kind, targets in (("native", NATIVE), ("docker", DOCKER)):
        for platform, target in targets.items():
            output = fetch(
                needs[f"{kind}-{platform}"]["outputs"]["artifact-id"],
                f"{kind}-{platform}",
            )
            receipt = check_output(output, kind, target, context(), manifest_sha)
            if kind == "native":
                native_files.update(
                    {name: f"{kind}-{platform}/{name}" for name in receipt["files"]}
                )
    destinations.append(
        {"repository": REPOSITORY, "tag": f"v{version()}", "files": native_files}
    )
    require(
        len({producer["id"] for producer in producers}) == len(producers),
        "Duplicate producer artifact IDs",
    )
    # File paths, not basenames, identify assets shared by different releases.
    files = {
        path: digest(safe_file(directory, path))
        for destination in destinations
        for path in destination["files"].values()
    }
    write(
        directory / "plan.json",
        {
            "context": context(),
            "manifest_sha256": manifest_sha,
            "producers": producers,
            "destinations": destinations,
            "files": files,
        },
    )


def check_output(directory, kind, target, expected, manifest_sha):
    receipt = load(directory / "output.json")
    require(
        receipt["kind"] == kind and receipt["target"] == target,
        "Wrong executable output role",
    )
    require(
        all(
            receipt["context"].get(key) == value
            for key, value in expected.items()
            if key != "attempt"
        ),
        "Executable output belongs to another source or run",
    )
    require(
        0 < receipt["context"]["attempt"] <= expected["attempt"],
        "Invalid producer attempt",
    )
    require(
        receipt["manifest_sha256"] == manifest_sha,
        "Executable embeds another runtime manifest",
    )
    require(
        set(receipt["files"]) == expected_output(kind, target),
        "Incomplete executable output",
    )
    verify_files(directory, receipt["files"])
    return receipt


def builtin_destinations(builtins, root):
    result = []
    for entry in load("builtin-artifacts.json").values():
        component = entry["component"]
        package = builtins / component
        descriptor = load(package / "package.json")
        result.append(
            {
                "repository": BUILTINS,
                "tag": f"{component}-v{version()}",
                "notes": (package / "release-notes.md").relative_to(root).as_posix(),
                "files": {
                    name: (package / name).relative_to(root).as_posix()
                    for name in descriptor["files"]
                    if name != "release-notes.md"
                },
            }
        )
    return result


def release(repository, tag):
    published = api(
        f"repos/{repository}/releases/tags/{quote(tag, safe='')}", missing=True
    )
    if published is not None:
        return published
    # Tag lookup only returns published releases. Authenticated listing includes drafts.
    matches = [
        item
        for item in pages(f"repos/{repository}/releases")
        if item["tag_name"] == tag
    ]
    require(len(matches) <= 1, f"Ambiguous release tag: {repository}@{tag}")
    return matches[0] if matches else None


def compare_public(destination, files, root, allow_missing):
    remote = release(destination["repository"], destination["tag"])
    if remote is None:
        require(allow_missing, f"Missing release: {destination['tag']}")
        return list(destination["files"])
    if destination["repository"] == REPOSITORY and allow_missing:
        require(remote["draft"], "Preparation requires a draft Golem release")
    assets = {asset["name"]: asset for asset in remote["assets"]}
    require(len(assets) == len(remote["assets"]), "Ambiguous release assets")
    missing = []
    with tempfile.TemporaryDirectory() as temporary:
        for name, path in destination["files"].items():
            if name not in assets:
                require(
                    allow_missing, f"Missing public asset: {destination['tag']}/{name}"
                )
                missing.append(name)
                continue
            downloaded = Path(temporary) / name
            run(
                "gh",
                "release",
                "download",
                destination["tag"],
                "--repo",
                destination["repository"],
                "--pattern",
                name,
                "--dir",
                temporary,
            )
            require(
                digest(downloaded) == files[path],
                f"Conflicting immutable asset: {destination['tag']}/{name}; no assets were replaced",
            )
            downloaded.unlink()
    return missing


def publish(directory, builtins_only=False):
    if builtins_only:
        check_bundle(directory)
        destinations = builtin_destinations(directory, directory)
        destinations.append(
            {
                "repository": REPOSITORY,
                "tag": f"v{version()}",
                "files": {MANIFEST: MANIFEST},
            }
        )
        plan = {
            "context": context(),
            "destinations": destinations,
            "files": {
                path: digest(safe_file(directory, path))
                for dest in destinations
                for path in dest["files"].values()
            },
        }
    else:
        plan = load(directory / "plan.json")
        require(
            plan["context"] == context(),
            "Publication plan belongs to another source, run, or attempt",
        )
        verify_files(directory, plan["files"])
    # Inspect the complete remote set before the first create or upload.
    missing = [
        compare_public(dest, plan["files"], directory, True)
        for dest in plan["destinations"]
    ]
    for dest, names in zip(plan["destinations"], missing):
        if release(dest["repository"], dest["tag"]) is None:
            command = [
                "gh",
                "release",
                "create",
                dest["tag"],
                "--repo",
                dest["repository"],
                "--title",
                dest["tag"],
            ]
            if dest["repository"] == REPOSITORY:
                command += [
                    "--verify-tag",
                    "--draft",
                    "--generate-notes",
                    f"--prerelease={'-' in version()}",
                ]
            else:
                command += ["--notes-file", str(directory / dest["notes"])]
            run(*command)
        for name in names:
            run(
                "gh",
                "release",
                "upload",
                dest["tag"],
                str(directory / dest["files"][name]),
                "--repo",
                dest["repository"],
            )
    for dest in plan["destinations"]:
        compare_public(dest, plan["files"], directory, False)
    if not builtins_only:
        draft = release(REPOSITORY, f"v{version()}")
        require(
            draft["draft"],
            "Golem was published during preparation; readiness was not issued",
        )
        write(directory / "release-ready.json", {**plan, "release_id": draft["id"]})


def readiness(tag, directory, published=False):
    require(tag == f"v{version()}", "Readiness tag does not match the selected version")
    remote = release(REPOSITORY, tag)
    require(
        remote is not None and remote["draft"] != published,
        "Unexpected Golem draft/publication state",
    )
    if published and os.environ.get("CI"):
        require(
            str(remote["id"]) == os.environ.get("RELEASE_EVENT_ID"),
            "Publication event belongs to another release",
        )
    sha = subprocess.check_output(
        ["git", "rev-parse", f"{tag}^{{commit}}"], text=True
    ).strip()
    if published and os.environ.get("CI"):
        require(
            os.environ.get("GITHUB_SHA") == sha,
            "Publication event belongs to another source",
        )
    ref = api(f"repos/{REPOSITORY}/git/ref/tags/{quote(tag, safe='')}")["object"]
    for _ in range(10):
        if ref["type"] == "commit":
            break
        require(ref["type"] == "tag", "Invalid public release ref")
        ref = api(f"repos/{REPOSITORY}/git/tags/{ref['sha']}")["object"]
    require(
        ref["type"] == "commit" and ref["sha"] == sha,
        "Public tag moved or differs from local tag",
    )
    candidates = []
    endpoint = f"repos/{REPOSITORY}/actions/workflows/prepare-golem-release.yaml/runs?event=workflow_dispatch&head_sha={sha}&status=success"
    for workflow_run in pages(endpoint, "workflow_runs"):
        require(
            workflow_run["repository"]["full_name"] == REPOSITORY,
            "Unexpected workflow repository",
        )
        if (
            workflow_run["path"] != WORKFLOW
            or workflow_run["head_sha"] != sha
            or workflow_run["head_branch"] != tag
        ):
            continue
        attempt = api(
            f"repos/{REPOSITORY}/actions/runs/{workflow_run['id']}/attempts/{workflow_run['run_attempt']}"
        )
        if attempt["conclusion"] != "success" or attempt["status"] != "completed":
            continue
        if published and attempt["updated_at"] >= remote["published_at"]:
            continue
        name = f"release-ready-{workflow_run['id']}-{workflow_run['run_attempt']}"
        artifacts = [
            item
            for item in pages(
                f"repos/{REPOSITORY}/actions/runs/{workflow_run['id']}/artifacts",
                "artifacts",
            )
            if item["name"] == name
        ]
        if len(artifacts) == 1 and not artifacts[0]["expired"]:
            candidates.append((workflow_run, artifacts[0]))
    require(
        len(candidates) == 1,
        "Expected exactly one successful tagged preparation with unexpired readiness evidence",
    )
    workflow_run, artifact = candidates[0]
    download(check_artifact(artifact["id"], workflow_run["id"], sha), directory)
    ready = load(directory / "release-ready.json")
    expected = {
        "repository": REPOSITORY,
        "ref": f"refs/tags/{tag}",
        "sha": sha,
        "run_id": workflow_run["id"],
        "attempt": workflow_run["run_attempt"],
    }
    require(
        ready["context"] == expected and ready["release_id"] == remote["id"],
        "Readiness belongs to another ref, attempt, or release",
    )
    require(
        not published or artifact["created_at"] < remote["published_at"],
        "Release was published before readiness",
    )
    for producer in ready["producers"]:
        current = check_artifact(producer["id"], workflow_run["id"], sha)
        require(
            current["digest"] == producer["digest"]
            and current["name"] == producer["name"],
            "Producer artifact changed",
        )
    for dest in ready["destinations"]:
        compare_public(dest, ready["files"], directory, False)
    print(
        f"Release {tag} is ready from preparation run {workflow_run['id']}, attempt {workflow_run['run_attempt']}"
    )
    return ready


def docker_payloads(directory):
    ready = load(directory / "release-ready.json")
    for platform, target in DOCKER.items():
        matches = [
            item
            for item in ready["producers"]
            if item["name"].startswith(f"docker-{target}-{ready['context']['run_id']}-")
        ]
        require(len(matches) == 1, f"Missing or ambiguous Docker payload: {target}")
        artifact = check_artifact(
            matches[0]["id"], ready["context"]["run_id"], ready["context"]["sha"]
        )
        require(artifact["digest"] == matches[0]["digest"], "Docker artifact changed")
        output = directory / platform
        download(artifact, output)
        check_output(
            output, "docker", target, ready["context"], ready["manifest_sha256"]
        )
        for name in expected_output("docker", target):
            with tarfile.open(output / name) as archive:
                for member in archive.getmembers():
                    require(
                        member.isfile()
                        and member.name.startswith(f"{target}/release/")
                        and ".." not in Path(member.name).parts,
                        "Unsafe Docker payload",
                    )
                archive.extractall("target", filter="data")


def check_start():
    require(
        sys.version_info >= (3, 14), "Release packaging requires Python 3.14 or newer"
    )
    for tool in ("git", "gh", "cargo", "wasm-tools"):
        require(shutil.which(tool), f"Missing release prerequisite: {tool}")
    require(
        not subprocess.check_output(
            ["git", "status", "--porcelain", "--untracked-files=all"], text=True
        ).strip(),
        "Release requires clean committed source",
    )
    run("gh", "auth", "status")
    require(
        release(REPOSITORY, f"v{version()}") is None,
        "Release version already exists; inspect it before resuming",
    )
    require(
        api(f"repos/{REPOSITORY}/git/ref/tags/v{version()}", missing=True) is None,
        "Release tag already exists; inspect it before resuming",
    )
    environment = api(f"repos/{REPOSITORY}/environments/golem-release")
    require(
        environment["deployment_branch_policy"]
        == {"protected_branches": False, "custom_branch_policies": True},
        "golem-release must restrict deployment tags",
    )
    policies = list(
        pages(
            f"repos/{REPOSITORY}/environments/golem-release/deployment-branch-policies",
            "branch_policies",
        )
    )
    require(
        policies and all(policy["type"] == "tag" for policy in policies),
        "golem-release must permit only tags",
    )
    require(
        any(
            fnmatch.fnmatchcase(f"v{version()}", policy["name"]) for policy in policies
        ),
        "golem-release does not permit the selected release tag",
    )
    variables = api(f"repos/{REPOSITORY}/environments/golem-release/variables")[
        "variables"
    ]
    require(
        any(
            item["name"] == "RELEASE_APP_ID" and item["value"].isdigit()
            for item in variables
        ),
        "Missing RELEASE_APP_ID",
    )
    secrets = api(f"repos/{REPOSITORY}/environments/golem-release/secrets")["secrets"]
    require(
        any(item["name"] == "RELEASE_APP_PRIVATE_KEY" for item in secrets),
        "Missing RELEASE_APP_PRIVATE_KEY",
    )
    print(
        "Read-only release prerequisites passed. App installation/key validity and registry availability still require their publication gates."
    )


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    commands.add_parser("check-start")
    commands.add_parser("check-manifest")
    artifact = commands.add_parser("check-artifact")
    artifact.add_argument("--id", required=True)
    for name in (
        "seal-component",
        "package-builtins",
        "publish-builtins",
        "collect",
        "publish",
        "readiness",
        "docker-payloads",
        "seal-output",
    ):
        command = commands.add_parser(name)
        command.add_argument("--directory", required=True, type=Path)
        if name == "package-builtins":
            command.add_argument("--target", required=True, type=Path)
        if name == "seal-output":
            command.add_argument("--kind", required=True, choices=("native", "docker"))
            command.add_argument("--target", required=True)
        if name == "readiness":
            command.add_argument("--tag", required=True)
            command.add_argument("--published", action="store_true")
    args = parser.parse_args()
    if args.command == "check-start":
        check_start()
    elif args.command == "check-manifest":
        check_manifest()
    elif args.command == "check-artifact":
        check_artifact(args.id)
    elif args.command == "seal-component":
        seal_component(args.directory)
    elif args.command == "package-builtins":
        package_builtins(args.directory, args.target)
    elif args.command == "publish-builtins":
        publish(args.directory, builtins_only=True)
    elif args.command == "seal-output":
        seal_output(args.directory, args.kind, args.target)
    elif args.command == "collect":
        collect(args.directory)
    elif args.command == "publish":
        publish(args.directory)
    elif args.command == "readiness":
        readiness(args.tag, args.directory, args.published)
    elif args.command == "docker-payloads":
        docker_payloads(args.directory)


if __name__ == "__main__":
    try:
        main()
    except (ValueError, KeyError, OSError, subprocess.CalledProcessError) as error:
        sys.exit(f"Release stopped: {error}")
