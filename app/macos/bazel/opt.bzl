"""A wrapper rule that forces its dependencies into `-c opt`.

Why this exists: `//app/macos:install` is the *production* setup path —
it copies LoFi.app into `~/Applications` and registers a LaunchAgent, so
whatever it installs is the binary that runs at login, potentially for
weeks. Bazel's default compilation mode is `fastbuild` (unoptimized),
and nothing in `.bazelrc` overrides it, so a plain `bazel run
//app/macos:install` was shipping an unoptimized Swift + Rust build.

The alternative was a `--config=release` in `.bazelrc`, but that stays
opt-in: forgetting the flag silently installs a debug build with no
signal. An outgoing transition makes the guarantee structural — the
optimized build is a property of the `:install` target, not of how it
was invoked.

Only `:install` uses this. `:launch` deliberately keeps the default
fastbuild config: it is the dev cycle, where compile time matters more
than run time, and sharing the default config means `:launch` reuses the
same action cache as a bare `bazel build //...`.
"""

def _opt_transition_impl(_settings, _attr):
    return {"//command_line_option:compilation_mode": "opt"}

_opt_transition = transition(
    implementation = _opt_transition_impl,
    inputs = [],
    outputs = ["//command_line_option:compilation_mode"],
)

def _optimized_impl(ctx):
    files = depset(transitive = [t[DefaultInfo].files for t in ctx.attr.srcs])
    runfiles = ctx.runfiles()
    for t in ctx.attr.srcs:
        runfiles = runfiles.merge(t[DefaultInfo].default_runfiles)
    return [DefaultInfo(files = files, runfiles = runfiles)]

optimized = rule(
    implementation = _optimized_impl,
    doc = "Forwards the outputs of `srcs`, built with `--compilation_mode=opt`.",
    attrs = {
        "srcs": attr.label_list(
            cfg = _opt_transition,
            mandatory = True,
            doc = "Targets to rebuild in opt mode and re-expose.",
        ),
    },
)
