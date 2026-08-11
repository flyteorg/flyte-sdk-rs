# Releasing

Two artifacts ship from this repo, to two different registries:

| Artifact | Registry | What it is | Tag |
| --- | --- | --- | --- |
| `flyteplugins-rs` | PyPI | the launcher — declares the Flyte task, builds the worker image | `py-v*` |
| `flyte`, `flyte-macros` | crates.io | the worker SDK — compiled into the binary the container runs | `rs-v*` |

## Do the versions need to be in sync?

**No, and they should not be.** They are separate release trains on separate
tags.

What actually couples the two halves is the **descriptor contract**, not the
version number. `#[flyte::task]` emits `flyte_interface_version` in the
descriptor that `<binary> describe-interface` prints; the launcher checks it
against `SUPPORTED_DESCRIPTOR_VERSION` and refuses to run on a mismatch, naming
both sides:

```
gated_deploy speaks interface version 2, this launcher supports 1
 — update flyteplugins-rs or rebuild the worker
```

That check is the compatibility gate. Matching version numbers would add nothing
on top of it — `flyteplugins-rs` 0.4.0 paired with `flyte` 0.4.0 is no safer than
0.4.0 with 0.3.0, because neither pairing is verified by the version. Meanwhile
lockstep costs real things: a typo fix in the Python launcher would force a Rust
release with no Rust changes, and vice versa, and every empty release burns a
version number on crates.io that can never be reused or yanked back into
availability.

So: **bump each package when its own code changes, and bump
`flyte_interface_version` on both sides when the wire contract changes.**

There is one contract that is *not* versioned today, and it is worth knowing
about: the worker's argument list. `RustWorkerTask.container_args` emits
`a0 --inputs … --outputs-path … --run-name … --name …`, and the worker's parser
has to accept exactly that. Nothing checks it at runtime. If you change the
worker's CLI, treat it as a descriptor-version bump even though only the args
moved — otherwise an old launcher fails inside the container with an arg-parse
error instead of a clear message at launch.

Adopt lockstep only if that arg contract starts churning often enough that
"which pairs work" stops being obvious. Today it does not.

## Cutting a Python release

1. Bump `version` in `python/flyteplugins-rs/pyproject.toml`.
2. Merge that to `main`.
3. Tag and push:

   ```bash
   git tag py-v0.2.0 && git push origin py-v0.2.0
   ```

`release-python.yml` lints, builds the sdist and wheel, verifies the tag matches
`pyproject.toml`, asserts `rust.dockerignore` is actually inside both artifacts,
installs the wheel into a clean venv and imports it — then uploads to PyPI.

The `rust.dockerignore` assertion is not ceremony. That file is force-included
through `[tool.hatch.build.targets.wheel.force-include]` and is not a `.py` file,
so a packaging change can drop it silently. A wheel without it gives users
multi-GB build contexts and a full image rebuild on every run.

To rehearse without publishing, run the workflow manually from the Actions tab
with `dry_run` checked: everything runs except the upload.

## Cutting a Rust release

> **Blocked today.** See below.

1. Bump `version` under `[workspace.package]` in the root `Cargo.toml` (both
   crates inherit it).
2. Merge to `main`.
3. Tag and push:

   ```bash
   git tag rs-v0.2.0 && git push origin rs-v0.2.0
   ```

`release-rust.yml` runs `cargo fmt --check`, clippy with `-D warnings`, the test
suite, a tag/manifest version check, and a dry-run publish of each crate — then
publishes `flyte-macros` before `flyte`, which is the required order since
`flyte` depends on it.

### The `flyte_core` blocker

`crates/flyte` depends on `flyte_core` by git rev:

```toml
flyte_core = { git = "https://github.com/flyteorg/flyte-sdk", rev = "580a35c…" }
```

crates.io does not accept crates with git dependencies. `cargo publish` fails
with:

```
all dependencies must have a version requirement specified when publishing.
dependency `flyte_core` does not specify a version
```

`flyte-macros` publishes cleanly today; `flyte` cannot until one of these
happens:

- **`flyte_core` is published to crates.io** (the name is currently unclaimed),
  and the dependency becomes a normal version requirement; or
- **the pure-Rust controller swap lands**, which removes the `flyte_core`/pyo3
  dependency altogether — the outcome the code comments already anticipate.

The workflow deliberately fails at the dry-run step rather than letting a tag
imply a release that cannot ship.

## One-time setup

**PyPI (Trusted Publishing).** No API token is stored. On PyPI, add a trusted
publisher for the project: owner `flyteorg`, repository `flyte-sdk-rs`, workflow
`release-python.yml`, environment `pypi`. Then create a GitHub environment named
`pypi` (Settings → Environments). Until the publisher exists on PyPI the upload
step 403s; the build job still passes, so a dry run tells you nothing about
whether this is configured.

For the very first release, use PyPI's "pending publisher" flow — it lets you
register the publisher before the project exists.

**crates.io.** Create an API token with publish scope and store it as the
`CARGO_REGISTRY_TOKEN` repository secret, plus a GitHub environment named
`crates-io`. Scope the token to `flyte` and `flyte-macros` if you can; crates.io
supports per-crate token scoping.

Putting both publish jobs behind environments means you can require a reviewer
before anything is uploaded — worth switching on, since neither registry allows
re-uploading a version.
