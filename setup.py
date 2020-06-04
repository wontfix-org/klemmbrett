#!/usr/bin/env python

import setuptools as _setuptools

_setuptools.setup(
    name = "klemmbrett",
    version = "0.3.2",
    description = "Versatile clipboard manager",
    author = "Michael van Bracht",
    author_email = "michael@wontfix.org",
    url = "https://github.com/wontfix-org/klemmbrett",
    license="MIT",
    packages = _setuptools.find_packages(),
    scripts = ["scripts/klemmbrett"],
    # XXX(mvb): This is probably too os/distribution dependent and
    # should be moved to the actual packaging code
    long_description=open("README.md").read(),
    long_description_content_type="text/markdown",
    data_files = [
        ("/etc", ["conf/klemmbrett.conf"]),
        ("/etc/xdg/autostart/", ["conf/klemmbrett-autostart.desktop"]),
        ("share/klemmbrett/", ["data/gtk-paste.svg", "data/gtk-paste2.svg"]),
#        ("share/applications/", ["conf/klemmbrett.desktop"]),
    ],
    include_package_data=True,
    install_requires = [
        "notify2",
        "dbus-python",
        "pygobject",
        "pycrypto",
    ],
    classifiers=[
        "Programming Language :: Python :: 3 :: Only",
        "Environment :: X11 Applications :: GTK"
    ],
)
