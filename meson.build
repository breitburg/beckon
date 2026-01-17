project(
    'com.github.breitburg.elementary-intelligence',
    'vala', 'c',
    version: '1.0.0',
    meson_version: '>= 0.58.0'
)

gnome = import('gnome')
i18n = import('i18n')

add_global_arguments('-DGETTEXT_PACKAGE="@0@"'.format(meson.project_name()), language:'c')

subdir('data')
subdir('src')

gnome.post_install(glib_compile_schemas: true)
