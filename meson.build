# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 breitburg

project(
    'com.github.breitburg.beckon',
    version: '1.0.0',
    meson_version: '>= 0.59',
)

app_id = meson.project_name()
cargo = find_program('cargo')
cargo_sh = find_program('build-aux' / 'cargo.sh')

# Build the Rust binary with cargo and install it under the app id, so it
# matches the desktop file's Exec/Icon.
custom_target(
    'cargo-build',
    output: app_id,
    console: true,
    install: true,
    install_dir: get_option('bindir'),
    build_always_stale: true,
    command: [
        cargo_sh,
        meson.project_source_root(),
        '@OUTPUT@',
        meson.project_build_root() / 'cargo-target',
    ],
)

subdir('data')
