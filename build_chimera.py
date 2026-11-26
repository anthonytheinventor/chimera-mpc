"""Build CHIMERA MPC: the Rust engine for every target, then the SDK's own tools/build.py.

    python "plugins/CHIMERA MPC/build_chimera.py" [build.py options...]

The SDK's build compiles the .cpp files in src/ and links nothing else, so this script builds the
engine (engine/, a Rust static library) for each target first and hands build.py the right library
for each link step, by wrapping its Builder's link methods. The SDK itself is not modified.

Targets:
  arm32   armv7-unknown-linux-gnueabihf, nightly Rust with the NEON airway (--features neon)
  arm64   aarch64-unknown-linux-gnu
  native, desktop (this computer, zig's x86_64-windows-gnu on Windows): x86_64-pc-windows-gnu

Needs: rustup with the stable and nightly toolchains and those targets
(rustup target add x86_64-pc-windows-gnu aarch64-unknown-linux-gnu;
 rustup +nightly target add armv7-unknown-linux-gnueabihf).
"""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
SDK = os.path.abspath(os.path.join(HERE, "..", ".."))
ENGINE = os.path.join(HERE, "engine")
TARGET_DIR = os.environ.get("CHIMERA_TARGET_DIR", os.path.join(HERE, "build", "engine-target"))

# (rust target, toolchain, extra cargo args, RUSTFLAGS, system libraries for the final link)
LINUX_LIBS = ["-lutil", "-lrt", "-lpthread", "-lm", "-ldl", "-lc"]
RUST = {
    "arm32": ("armv7-unknown-linux-gnueabihf", "nightly", ["--features", "neon"], "-C target-cpu=cortex-a17", LINUX_LIBS),
    "arm64": ("aarch64-unknown-linux-gnu", "stable", [], "-C target-cpu=cortex-a55", LINUX_LIBS),
    "native": ("x86_64-pc-windows-gnu", "stable", [], "", ["-lkernel32", "-lntdll", "-luserenv", "-lws2_32", "-ldbghelp"]),
}


def engine_lib(kind):
    target = RUST[kind][0]
    return os.path.join(TARGET_DIR, target, "release", "libchimera_mpc_engine.a")


def build_engine(kind):
    target, toolchain, extra, rustflags, _ = RUST[kind]
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = TARGET_DIR
    if rustflags:
        env["RUSTFLAGS"] = rustflags
    else:
        env.pop("RUSTFLAGS", None)
    argv = ["cargo", "+" + toolchain, "build", "--release", "--target", target] + extra
    print("[engine] %s: %s" % (kind, " ".join(argv)), flush=True)
    r = subprocess.run(argv, cwd=ENGINE, env=env)
    if r.returncode != 0:
        sys.exit("engine build failed for %s" % kind)


def main():
    if os.name != "nt":
        sys.exit("this script builds the desktop engine for Windows only so far")
    for kind in RUST:
        build_engine(kind)
    sys.path.insert(0, os.path.join(SDK, "tools"))
    import build as sdk  # the SDK's tools/build.py

    def with_engine(method, kind_of):
        def wrapped(self, *args, **kwargs):
            kind = kind_of(*args)
            saved = self.sources
            self.sources = saved + [engine_lib(kind)] + RUST[kind][4]
            try:
                return method(self, *args, **kwargs)
            finally:
                self.sources = saved
        return wrapped

    B = sdk.Builder
    B.build_arm = with_engine(B.build_arm, lambda target, *rest: target)
    B.build_native = with_engine(B.build_native, lambda *rest: "native")
    B.build_desktop = with_engine(B.build_desktop, lambda *rest: "native")
    project = HERE
    return sdk.main([project] + sys.argv[1:])


if __name__ == "__main__":
    sys.exit(main())
