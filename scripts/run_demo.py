"""Launch the Rust hello-trace worker as a real task on a Union/Flyte v2 cluster.

The task container runs the Rust binary directly (no Python runtime involved in
the task): the backend templates the standard worker args ({{.input}},
{{.outputPrefix}}, {{.runName}}, {{.actionName}}) and injects the in-cluster
env (API key, _U_RUN_BASE, org/project/domain), which is exactly the contract
flyte::worker_main speaks. Traces recorded by the Rust SDK then show up as
child actions of the run in the console.

Usage (from this repo, using the Python SDK checkout next door):
    uv run --project ../flyte-sdk python scripts/run_demo.py \
        --image ghcr.io/unionai/flyte-sdk-rs-demo:v1 \
        [--config ~/.flyte/demo-config.yaml] [--project flytesnacks] [--domain development]
"""

import argparse
from dataclasses import dataclass
from pathlib import Path

import flyte
from flyte._task import TaskTemplate
from flyte.models import NativeInterface, SerializationContext


@dataclass(kw_only=True)
class RustWorkerTask(TaskTemplate):
    """A plain-container task speaking the standard worker arg contract."""

    async def execute(self, *args, **kwargs):
        raise NotImplementedError("remote-only task")

    def container_args(self, sctx: SerializationContext):
        return [
            "a0",
            "--inputs",
            sctx.input_path,
            "--outputs-path",
            sctx.output_path,
            "--run-name",
            "{{.runName}}",
            "--name",
            "{{.actionName}}",
        ]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", required=True, help="worker image URI (see scripts/build-image.sh)")
    parser.add_argument("--config", default=str(Path.home() / ".flyte/demo-config.yaml"))
    parser.add_argument("--project", default="flytesnacks")
    parser.add_argument("--domain", default="development")
    parser.add_argument("--x", type=int, default=21)
    parser.add_argument("--label", default="demo")
    args = parser.parse_args()

    task = RustWorkerTask(
        name="rust_trace_demo",
        # from_base = image already exists in a registry; nothing to build.
        image=flyte.Image.from_base(args.image),
        interface=NativeInterface(inputs={"x": (int, None), "label": (str, None)}, outputs={"o0": str}),
    )
    flyte.TaskEnvironment.from_task("rust_trace_env", task)

    flyte.init_from_config(args.config, project=args.project, domain=args.domain)
    run = flyte.run(task, x=args.x, label=args.label)
    print(f"run: {run.name}")
    print(f"url: {run.url}")
    run.wait()
    print(f"outputs: {run.outputs()}")


if __name__ == "__main__":
    main()
