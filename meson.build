# Not meant for standalone usage! The crate is still compiled via cargo!
# Only provided for easing integration of the library in meson parent projects.
# Exposing cargo's output as a prebuilt binary dependency

# Example subproject integration:
# nic_emu = subproject('nic-emu')
# nic_emu_dep = nic_emu.get_variable('nic_emu_dep')
# executable(..., dependencies : [nic_emu_dep, ...])

project(
    'nic-emu',
    version: '0.0.1',
    license: 'MIT',
)

if not meson.is_subproject()
    warning('nic-emu is not a standalone meson project!')
endif

cargo = find_program('cargo')
cargo_command = [
    cargo,
    'build',
    '--lib',
    '--no-default-features',
    '--features',
    'generate-bindings'
]

buildtype = get_option('buildtype')
rust_buildtype = 'debug'
if buildtype == 'release' or buildtype == 'minsize'
    rust_buildtype = 'release'
    cargo_command += '--release'
endif

message('Building nic-emu using cargo')
run_command(
    cargo_command,
    check: true
)

nic_emu_build_path = 'target' / rust_buildtype

cc = meson.get_compiler('cpp')
nic_emu_lib = cc.find_library(
    'nic_emu',
    dirs: meson.current_source_dir() / nic_emu_build_path,
)

# Include this dependency in your executable
nic_emu_dep = declare_dependency(
    dependencies: [nic_emu_lib],
    include_directories: include_directories(nic_emu_build_path / 'include'),
)
