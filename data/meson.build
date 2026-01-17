icon_sizes = ['16', '24', '32', '48', '64', '128']

foreach i : icon_sizes
    install_data(
        'icons' / i + '.svg',
        install_dir: get_option('datadir') / 'icons' / 'hicolor' / i + 'x' + i / 'apps',
        rename: meson.project_name() + '.svg'
    )
    install_data(
        'icons' / i + '.svg',
        install_dir: get_option('datadir') / 'icons' / 'hicolor' / i + 'x' + i + '@2' / 'apps',
        rename: meson.project_name() + '.svg'
    )
endforeach

install_data(
    'com.github.breitburg.elementary-intelligence.desktop',
    install_dir: get_option('datadir') / 'applications'
)

install_data(
    'com.github.breitburg.elementary-intelligence.metainfo.xml',
    install_dir: get_option('datadir') / 'metainfo'
)

install_data(
    'com.github.breitburg.elementary-intelligence.gschema.xml',
    install_dir: get_option('datadir') / 'glib-2.0' / 'schemas'
)
