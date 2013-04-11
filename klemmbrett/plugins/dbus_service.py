# coding: utf-8
"""
===========================
DBus service for Klemmbrett
===========================

The `DBusServicePlugin` allows other applications to inject
things into your history via dbus.

You can activate this plugin by adding the following to your
`klemmbrett.conf` file:

::
    [plugin dbus]
    plugin = klemmbrett.plugins.dbus_service.DBusServicePlugin

If you want to inject things into your history you can use the
`Klemmbrett` class from this file like this:

    >>> from klemmbrett.plugins.dbus_service import Klemmbrett
    >>> klemmbrett = Klemmbrett()
    >>> klemmbrett.add("TEXT FOR CLIPBOARD")
"""

import dbus
import dbus.glib # backwards compatible setting of mainloop
import dbus.service

from klemmbrett import plugins as _plugins

DBUS_NAME = "org.wontfix.Klemmbrett"
DBUS_PATH = "/org/wontfix/Klemmbrett"


class Klemmbrett(object):
    """
    Simple helper class to get the DBus proxy object. All calls
    to an instance are proxied to the underlying dbus object.
    """
    def __init__(self):
        bus = dbus.SessionBus()
        self.proxy = bus.get_object(DBUS_NAME, DBUS_PATH)

    def __getattr__(self, name):
        return getattr(self.proxy, name)


class DBusService(dbus.service.Object):
    """
    DBus service class for klemmbrett.

    This had to go to a seperate class to solve the famous
    `TypeError: metaclass conflict` you get if you inherit from
    `klemmbrett.plugins.Plugin` and `dbus.service.Object`.
    """
    def __init__(self, plugin):
        self.plugin = plugin
        bus_name = dbus.service.BusName(DBUS_NAME, bus=dbus.SessionBus())
        dbus.service.Object.__init__(self, bus_name, DBUS_PATH)

    @dbus.service.method(dbus_interface=DBUS_NAME, in_signature='s')
    def add(self, text):
        self.plugin.add(text)


class DBusServicePlugin(_plugins.Plugin):
    OPTIONS = {
        "tie:history": "history",
    }

    def __init__(self, name, options, klemmbrett):
        _plugins.Plugin.__init__(self, name, options, klemmbrett)
        self.dbus = DBusService(self)

    def add(self, text):
        self.history.set(text = text)

