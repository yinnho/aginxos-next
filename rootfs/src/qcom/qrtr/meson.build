# SPDX-License-Identifier: BSD-3-Clause

project('qrtr',
        'c',
        version: '1.2',
        license : [ 'BSD-3-Clause'],
        default_options : [
        'warning_level=1',
        'buildtype=release',
        ])

prefix = get_option('prefix')

inc = include_directories('include')
subdir('lib')
subdir('include')
subdir('src')
