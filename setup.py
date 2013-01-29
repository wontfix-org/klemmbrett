#!/usr/bin/env python

import setuptools as _setuptools

_setuptools.setup(
    name = "klemmbrett",
    version = "0.1-dev",
    description = "Versatile clipboard manager",
    author = "Michael van Bracht",
    author_email = "michael@wontfix.org",
    url = "https://github.com/wontfix-org/klemmbrett",
    packages = _setuptools.find_packages(),
    scripts = ["scripts/klemmbrett"],
    data_files = [
        ("/etc", ["conf/klemmbrett.conf"])
    ]
)
