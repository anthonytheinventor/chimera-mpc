"""Record the arm32 engine's profile for profile-guided optimization (PGO).

    python "plugins/CHIMERA MPC/tools/pgo_profile.py"

build_chimera.py builds the arm32 engine with engine/pgo/chimera-arm32.profdata. That profile
describes the engine's code as it was when it was recorded, so record it again after changing the
engine or CHIMERA's code (build_chimera.py says when it is out of date). It takes about two minutes:

  1. the arm32 engine built with -Cprofile-generate (build/pgo-gen), same flags otherwise;
  2. tools/pgo/train.c linked with it and run under qemu-arm (tools/pgo/train.sh, in WSL): Sing on
     the three bodies, Speak, Play with the vowel pad and a Blend, Beatbox with every beat preset,
     pads, hums, Mic FX and the host's transport;
  3. the raw profile merged with the nightly toolchain's llvm-profdata into the .profdata, and the
     sources it describes stamped beside it.

PGO changes how the code is laid out and inlined, not what it computes: a build with the profile
renders the same samples as one without (compare with train's second argument).

Needs: rustup nightly with armv7-unknown-linux-gnueabihf and the llvm-tools component
(rustup +nightly component add llvm-tools); WSL with arm-linux-gnueabihf-gcc and qemu-arm
(CHIMERA_WSL_DISTRO picks the distribution; default: WSL's default).
"""
import os
import subprocess
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
PROJECT = os.path.dirname(HERE)
sys.path.insert(0, PROJECT)
import build_chimera as bc  # noqa: E402

GEN_DIR = os.path.join(PROJECT, "build", "pgo-gen")


def wsl_path(p):
    p = os.path.abspath(p).replace("\\", "/")
    return "/mnt/" + p[0].lower() + p[2:] if len(p) > 1 and p[1] == ":" else p


def run(argv, **kw):
    print("[pgo] " + " ".join(argv), flush=True)
    r = subprocess.run(argv, **kw)
    if r.returncode != 0:
        sys.exit("[pgo] failed: " + argv[0])


def nightly_tool(name):
    sysroot = subprocess.run(["rustc", "+nightly", "--print", "sysroot"], capture_output=True, text=True).stdout.strip()
    host = [l.split(": ")[1] for l in subprocess.run(["rustc", "+nightly", "-vV"], capture_output=True, text=True).stdout.splitlines() if l.startswith("host:")][0]
    exe = os.path.join(sysroot, "lib", "rustlib", host, "bin", name + (".exe" if os.name == "nt" else ""))
    if not os.path.exists(exe):
        sys.exit("[pgo] %s not found: rustup +nightly component add llvm-tools" % name)
    return exe


def main():
    target, toolchain, extra, rustflags, _ = bc.RUST["arm32"]
    env = dict(os.environ)
    env["CARGO_TARGET_DIR"] = GEN_DIR
    env["RUSTFLAGS"] = rustflags + " -Cprofile-generate=/tmp/chimera-pgo"
    run(["cargo", "+" + toolchain, "build", "--release", "--target", target] + extra, cwd=bc.ENGINE, env=env)
    lib = os.path.join(GEN_DIR, target, "release", "libchimera_mpc_engine.a")

    libdir = subprocess.run(["rustc", "+" + toolchain, "--print", "target-libdir", "--target", target], capture_output=True, text=True).stdout.strip()
    rlibs = [f for f in os.listdir(libdir) if f.startswith("libprofiler_builtins-") and f.endswith(".rlib")]
    if not rlibs:
        sys.exit("[pgo] the %s toolchain has no profiler runtime for %s" % (toolchain, target))
    raw = os.path.join(GEN_DIR, "train.profraw")
    if os.path.exists(raw):
        os.remove(raw)
    wsl = ["wsl.exe"] + (["-d", os.environ["CHIMERA_WSL_DISTRO"]] if os.environ.get("CHIMERA_WSL_DISTRO") else []) + ["-e", "bash"]
    run(wsl + [wsl_path(os.path.join(HERE, "pgo", "train.sh")), wsl_path(lib), wsl_path(os.path.join(libdir, rlibs[0])),
               wsl_path(os.path.join(PROJECT, "Content")), wsl_path(raw)])

    os.makedirs(os.path.dirname(bc.PGO_PROFILE), exist_ok=True)
    run([nightly_tool("llvm-profdata"), "merge", "-o", bc.PGO_PROFILE, raw])
    with open(bc.PGO_STAMP, "w", newline="\n") as f:
        f.write(bc.sources_stamp() + "\n")
    print("[pgo] wrote %s (%d bytes)" % (bc.PGO_PROFILE, os.path.getsize(bc.PGO_PROFILE)))


if __name__ == "__main__":
    main()
