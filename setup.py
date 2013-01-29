#!/usr/bin/env python
from setuptools import setup, find_packages

setup(
    name = "klemmbrett",
    version = "0.1-dev",
    description = "Versatile clipboard manager",
    author = "Michael van Bracht",
    author_email = "michael@wontfix.org",
    url = "http://klemmbrett.wontfix.org",
    packages = find_packages(),
    scripts = ["scripts/klemmbrett"],
    data_files = [
        ("/etc", ["conf/klemmbrett.conf"])
    ]
)
