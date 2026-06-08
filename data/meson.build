# SPDX-License-Identifier: GPL-3.0-or-later
# SPDX-FileCopyrightText: 2026 breitburg

# Desktop entry
install_data(
    '@0@.desktop'.format(app_id),
    install_dir: get_option('datadir') / 'applications',
)

# AppStream metainfo
install_data(
    '@0@.metainfo.xml'.format(app_id),
    install_dir: get_option('datadir') / 'metainfo',
)

# Icons
foreach size : ['16', '24', '32', '48', '64', '128']
    install_data(
        'icons' / '@0@.svg'.format(size),
        install_dir: get_option('datadir') / 'icons' / 'hicolor' / '@0@x@0@'.format(size) / 'apps',
        rename: '@0@.svg'.format(app_id),
    )
endforeach
