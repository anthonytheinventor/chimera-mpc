"""Build CHIMERA MPC: the Rust engine for every target, then the SDK's own tools/build.py.

    python "plugins/CHIMERA MPC/build_chimera.py" [build.py options...]

The SDK's build compiles the .cpp files in src/ and links nothing else, so this script builds the
engine (engine/, a Rust static library) for each target first and hands build.py the right library
for each link step, by wrapping its Builder's link methods. The SDK itself is not modified.

Targets:
  arm32   armv7-unknown-linux-gnueabihf, nightly Rust with the NEON airway (--features neon)
  arm64   aarch64-unknown-linux-gnu
  native, desktop (this computer, zig's x86_64-windows-gnu on Windows): x86_64-pc-windows-gnu

The arm32 engine is built with profile-guided optimization from engine/pgo/chimera-arm32.profdata
(recorded by tools/pgo_profile.py; it says when that profile is older than the code).
CHIMERA_PGO=0 builds without it; CHIMERA_ARM32_PROFILE names another .profdata.

Needs: rustup with the stable and nightly toolchains and those targets
(rustup target add x86_64-pc-windows-gnu aarch64-unknown-linux-gnu;
 rustup +nightly target add armv7-unknown-linux-gnueabihf).
"""
import hashlib
import os
import re
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


PGO_PROFILE = os.path.join(ENGINE, "pgo", "chimera-arm32.profdata")
PGO_STAMP = os.path.join(ENGINE, "pgo", "sources.sha256")


def chimera_crate():
    """CHIMERA's engine crate, from engine/Cargo.toml's path dependency."""
    with open(os.path.join(ENGINE, "Cargo.toml"), encoding="utf-8") as f:
        m = re.search(r'^chimera\s*=\s*\{[^}]*path\s*=\s*"([^"]+)"', f.read(), re.M)
    return os.path.normpath(os.path.join(ENGINE, m.group(1))) if m else None


def sources_stamp():
    """One hash of the code a profile describes: the engine's and CHIMERA's Rust sources and
    manifests (line endings ignored, so a checkout's settings do not matter)."""
    h = hashlib.sha256()
    for root in [ENGINE, chimera_crate()]:
        if root is None or not os.path.isdir(root):
            continue
        files = []
        for d, dirs, names in os.walk(root):
            dirs[:] = [x for x in dirs if x not in ("target", "pgo", "tests", "examples", "benches")]
            files += [os.path.join(d, n) for n in names if n.endswith(".rs") or n in ("Cargo.toml", "Cargo.lock")]
        for path in sorted(files):
            h.update(os.path.relpath(path, root).replace("\\", "/").encode())
            with open(path, "rb") as f:
                h.update(f.read().replace(b"\r\n", b"\n"))
    return h.hexdigest()


def arm32_profile():
    """The .profdata the arm32 engine is built with, or None."""
    if os.environ.get("CHIMERA_PGO") == "0":
        return None
    profile = os.environ.get("CHIMERA_ARM32_PROFILE") or (PGO_PROFILE if os.path.exists(PGO_PROFILE) else None)
    if profile and os.path.abspath(profile) == PGO_PROFILE:
        stamp = open(PGO_STAMP).read().strip() if os.path.exists(PGO_STAMP) else ""
        if stamp != sources_stamp():
            print("[engine] note: the PGO profile was recorded from older engine or CHIMERA code; the build "
                  "is correct but gains less from it. Record it again: python tools/pgo_profile.py", flush=True)
    return profile


def engine_lib(kind):
    target = RUST[kind][0]
    return os.path.join(TARGET_DIR, target, "release", "libchimera_mpc_engine.a")


def build_engine(kind):
    target, toolchain, extra, rustflags, _ = RUST[kind]
    # profile-guided optimization of the arm32 engine (see arm32_profile)
    flags = rustflags.split()
    profile = arm32_profile() if kind == "arm32" else None
    if profile:
        print("[engine] arm32 with the PGO profile %s" % profile, flush=True)
        flags.append("-Cprofile-use=" + os.path.abspath(profile))
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = TARGET_DIR
    # Cargo's encoded form keeps a flag whole when its path has a space ("CHIMERA MPC")
    env.pop("RUSTFLAGS", None)
    if flags:
        env["CARGO_ENCODED_RUSTFLAGS"] = "\x1f".join(flags)
    else:
        env.pop("CARGO_ENCODED_RUSTFLAGS", None)
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
