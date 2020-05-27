#!/usr/bin/env python

import weakref as _weakref

import notify2 as _notify

import gi as _gi
_gi.require_version('Gtk', '3.0')
from gi.repository import Gtk as _gtk
_gi.require_version('Gdk', '3.0')
from gi.repository import Gdk as _gdk
from gi.repository import GObject as _gobject
_gi.require_version('Keybinder', '3.0')
from gi.repository import Keybinder as _keybinder

import klemmbrett.util as _util
import klemmbrett.config as _config

class KlemmbrettVormKopf(Exception):
    pass


class Klemmbrett(_gobject.GObject):

    __gsignals__ = {
        "text-selected": (_gobject.SIGNAL_RUN_FIRST, None, (_gobject.TYPE_PYOBJECT,)),
        "text-set": (_gobject.SIGNAL_RUN_FIRST, None, (_gobject.TYPE_PYOBJECT,)),
    }

    _PLUGIN_PREFIX = "plugin "
    _TIE_PREFIX = "tie:"

    def __init__(self, config_files):
        super(Klemmbrett, self).__init__()
        self._clipboard = _gtk.Clipboard.get(_gdk.SELECTION_CLIPBOARD)
        self._primary = _gtk.Clipboard.get(_gdk.SELECTION_PRIMARY)

        self.config = _config.Config()
        self.config.read(config_files)

        _keybinder.init()

        # configure klemmbrett
        self._plugins = dict()
        self._load_plugins()

        self._sync = _util.humanbool(self.config.get('klemmbrett', 'sync', True))

        self.selection = None

        self._clipboard.connect('owner-change', self._clipboard_owner_changed)
        self._primary.connect('owner-change', self._clipboard_owner_changed)

        _notify.init("Klemmbrett")

    def _load_plugins(self):
        for section in self.config.sections():
            if not section.startswith(self._PLUGIN_PREFIX):
                continue

            name = section[len(self._PLUGIN_PREFIX):].strip()
            opts = dict(self.config.items(section))
            # load the plugin module and create an instance
            plugin = _util.load_dotted(opts['plugin'])
            plugin = plugin(
                name,
                dict(plugin.OPTIONS, **opts),
                self,
            )
            self._plugins[name] = plugin


        # inject attributes into plugins on a per plugin config
        # basis via config options of the form:
        # tie:plugin_local_identitier = global_name
        # or from the plugin internal config dict.
        for name, plugin in self._plugins.items():
            for key, value in plugin.options.items():
                if not key.startswith(self._TIE_PREFIX):
                    continue

                source = key[len(self._TIE_PREFIX):].strip()
                target = _weakref.proxy(self._plugins[value])
                setattr(self._plugins[name], source, target)

        # only after alll plugins are instantiated and properly
        # injected, they may connect to other plugins signals,
        # so we, sadly, have to live with some kind of bootstrap
        # method with some additional code
        for plugin in self._plugins.values():
            plugin.bootstrap()

    def _clipboard_owner_changed(self, clipboard, event):
        text = clipboard.wait_for_text()

        if text != self.selection and text is not None:
            self.selection = text
            if self._sync:
                if clipboard is self._primary:
                    self._clipboard.set_text(text, -1)
                elif clipboard is self._clipboard:
                    self._primary.set_text(text, -1)

            self.emit("text-selected", text)

        return True

    def notify(self, summary, text):
        """ Display a message about the new suggestion and its origin """
        n = _notify.Notification(summary, text)
        n.show()

    def set(self, text, primary = True, clipboard = True):
        try:
            if clipboard:
                self._clipboard.set_text(text, -1)
            if primary:
                self._primary.set_text(text, -1)
            self.emit("text-set", text)
        except TypeError:
            # TypeError: Gtk.Clipboard.set_text() argument 1 must be string, not None
            # FIXME(mbra): i really do not know when and when this happens :/
            pass

    def main(self):
        _gtk.main()


