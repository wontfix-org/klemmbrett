#!/usr/bin/env python

import os as _os
import re as _re
import itertools as _it
import functools as _ft
import pickle as _pickle
import weakref as _weakref
import collections as _collections

import gi as _gi
_gi.require_version('Gtk', '3.0')
from gi.repository import Gtk as _gtk
_gi.require_version('Gdk', '3.0')
from gi.repository import Gdk as _gdk
from gi.repository import GdkX11 as _gdkx11
from gi.repository import GObject as _gobject
_gi.require_version('Keybinder', '3.0')
from gi.repository import Keybinder as _keybinder

import klemmbrett.util as _util
import klemmbrett as _klemmbrett
import klemmbrett.config as _config

import logging as _logging
_log = _logging.getLogger(__name__)


class HistoryEmpty(Exception):
    pass


class Plugin(_gobject.GObject):

    OPTIONS = {}

    def __init__(self, name, options, klemmbrett):
        if hasattr(self, '_initialized'):
            return

        _gobject.GObject.__init__(self)

        if type(klemmbrett) in _weakref.ProxyTypes:
            self.klemmbrett = _weakref.proxy(klemmbrett)
        else:
            self.klemmbrett = klemmbrett

        self.options = options
        self.name = name

    def set(self, widget = None, text = None):
        if callable(text):
            text = text()
        self.klemmbrett.set(text)

    def _printable(self, text, htmlsafe = False):
        ll = int(self.options.get('line-length', 30))
        clean = _re.sub(r'\s+', ' ', text).strip()

        if len(clean) > ll:
            om = self.options.get('omit-mode', 'middle')
            if om == 'start':
                clean = clean[len(clean) - ll:]
            elif om == 'middle':
                clean = clean[:int(ll / 2)] + " ... " + clean[len(clean) - int(ll / 2):]
            elif om == 'end':
                clean = clean[:ll]

        if htmlsafe:
            clean = _util.htmlsafe(clean)

        return clean

    def bootstrap(self):
        pass


class StatusIcon(Plugin):

    def __init__(self, *args, **kwargs):
        super(StatusIcon, self).__init__(*args, **kwargs)

        self.menu = _gtk.Menu()
        item = _gtk.MenuItem("Quit")
        item.connect('activate', _gtk.main_quit)
        self.menu.append(item)
        self.menu.show_all()

        self.tray = _gtk.StatusIcon()
        self.tray.set_visible(True)
        self.tray.set_tooltip_text("Klemmbrett")
        self.tray.connect('popup-menu', self.on_menu, self.menu)

        icon = self.options.get('icon-path', None)
        if icon:
            if _os.path.sep in icon:
                self.tray.set_from_file(_os.path.expanduser(icon))
            else:
                self.tray.set_from_icon_name(icon)
        else:
            self.tray.set_from_stock(_gtk.STOCK_ABOUT)

    def on_menu(self, icon, event_button, event_time, menu):
        menu.popup(
            None,
            None,
            _gtk.StatusIcon.position_menu,
            self.tray,
            event_button,
            event_time,
        )


class PopupPlugin(Plugin):

    def bootstrap(self):
        _keybinder.bind(
            self.options.get('shortcut', self.DEFAULT_BINDING),
            self.popup,
        )

    def _expand(self, widget, iterable):
        sm = widget.get_submenu()
        self._build_menu(sm, iterable())
        sm.show_all()

    def _build_menu(self, menu, items):
        accels = list(
            _it.chain(
                range(0, 10),
                map(
                    chr,
                    _it.chain(
                        range(ord('a'), ord('z') + 1),
                        range(ord('A'), ord('Z') + 1)),
                )
            )
        )
        for pos, (label, value) in enumerate(items):
            try:
                label = "_%s %s" % (accels[pos], label.replace('_', '__'))
                item = _gtk.MenuItem(label, use_underline = True)
            except IndexError:
                # We ran out of accelerator keys, too bad...
                item = _gtk.MenuItem(label, use_underline = False)

            if _util.isgenerator(value):
                item.set_submenu(_gtk.Menu())
                item.connect("activate", self._expand, value)
            else:
                item.connect("activate", self.set, value)

            menu.append(item)

    def _ungrab_keyboard(self):
        dm = _gdkx11.X11DeviceManagerCore(display=_gdk.Display.get_default())
        for dev in dm.list_devices(_gdk.DeviceType.MASTER):
            if dev.get_source() == _gdk.InputSource.KEYBOARD:
                dev.ungrab(_keybinder.get_current_event_time())

    def popup(self, keystr, items = None):
        self._ungrab_keyboard()

        menu = _gtk.Menu()
        index = 0

        if _util.humanbool(self.options.get('show-current-selection', 'yes')) and len(self.history):
            item = _gtk.MenuItem("")
            item.get_children()[0].set_markup(
                "<b>%s</b>" % (self._printable(self.history.top, True),),
            )
            menu.append(item)
            menu.append(_gtk.SeparatorMenuItem())
            index += 1

        self._build_menu(menu, items or self.items())

        menu.show_all()
        menu.popup(
            None,
            None,
            None,
            None,
            0,
            _keybinder.get_current_event_time(),
        )
        menu.set_active(index)
        return True


class FancyItemsMixin(object):

    SIMPLE_SECTION_DEFAULT = "value"
    _TYPE_SEPERATOR = "."

    def bootstrap(self):
        self._items = []
        section = self.options.get('simple-section', self.SIMPLE_SECTION)
        default_type = self.options.get('simple-section-default', self.SIMPLE_SECTION_DEFAULT)

        if not self.klemmbrett.config.has_section(section):
            raise KeyError("No config section %s defined" % (section,))

        for label, value in self.klemmbrett.config.items(section):
            target = default_type
            if self._TYPE_SEPERATOR in label:
                target, label = label.split(self._TYPE_SEPERATOR, 1)

            self._items.append((label, {target: value}))

        prefix = self.options.get('complex-section-prefix', self.COMPLEX_SECTION_PREFIX)

        for section in self.klemmbrett.config.sections():
            if not section.startswith(prefix):
                continue

            options = dict(self.klemmbrett.config.items(section))
            label = section[len(prefix):]

            self._items.append((label, options))

            if "shortcut" in options:
                _keybinder.bind(
                    options['shortcut'],
                    _ft.partial(self.set, widget = None, text = label),
                )

    def items(self):
        return iter(self._items)


class HistoryController(Plugin):

    __gsignals__ = {
        "text-accepted": (_gobject.SIGNAL_RUN_FIRST, None, (_gobject.TYPE_PYOBJECT,)),
    }

    def __init__(self, name, options, klemmbrett):
        Plugin.__init__(self, name, options, klemmbrett)
        self._history = _collections.deque(maxlen = self.maxlen)
        self._extend_detection = _util.humanbool(self.options.get('extend-detection', 'yes'))

    def items(self):
        for text in self._history:
            yield (
                self._printable(text),
                text,
            )

    def is_extended(self, text):
        if not self._extend_detection:
            return False

        if (
            len(self)
            and (
                text.startswith(self.top)
                or text.endswith(self.top)
            )
        ):

            return True
        return False

    def add(self, text, emit = True):
        if self.accepts(text):
            if self.is_extended(text):
                self._history[0] = text
            else:
                self._history.appendleft(text)

            if emit:
                self.emit("text-accepted", text)
            return True
        return False

    def __iter__(self):
        return iter(self._history)

    def __len__(self):
        return len(self._history)

    def accepts(self, text):
        # do not accept bullshit
        if not isinstance(text, str):
            return False

        # do not accept empty strings and pure whitespace strings
        text = text.strip()
        if not text:
            return False

        # accept everything if the history is empty
        if not len(self._history):
            return True

        # only if it is not the current selection
        return text != self.top

    @property
    def top(self):
        if not self._history:
            raise HistoryEmpty("The history is empty")
        return self._history[0]

    @property
    def maxlen(self):
        return int(self.options.get("length", 15))


class HistoryPicker(HistoryController, PopupPlugin):

    DEFAULT_BINDING = "<Ctrl><Alt>C"

    OPTIONS = {
        "tie:history": "history",
    }

    def __init__(self, name, options, klemmbrett):
        PopupPlugin.__init__(self, name, options, klemmbrett)
        HistoryController.__init__(self, name, options, klemmbrett)

    def bootstrap(self):
        PopupPlugin.bootstrap(self)
        HistoryController.bootstrap(self)
        self.klemmbrett.connect("text-selected", self._text_selected)

    def _text_selected(self, widget, text):
        return self.add(text)


class PersistentHistory(Plugin):
    OPTIONS = {
        "tie:history": "history",
    }

    def __init__(self, *args, **kwargs):
        super(PersistentHistory, self).__init__(*args, **kwargs)
        self._histfile = _os.path.expanduser(self.options.get("histfile", "~/.klemmbrett.history"))

    def bootstrap(self):
        self._load()
        self._persist = open(self._histfile, "a")
        self.history.connect("text-accepted", self._text_accepted)

    def _load(self):
        if not _os.path.exists(self._histfile):
            return

        history = open(self._histfile, "r")
        dq = _collections.deque(maxlen = self.history.maxlen)

        while True:
            try:
                dq.append(_pickle.load(history))
            except EOFError:
                break

        history.close()

        for i in dq:
            self.history.add(i, False)

    def _text_accepted(self, widget, text):
        # FIXME(mbra): we need to handle a history size limit, so the file does
        # not grow indefinitely
        _pickle.dump(text, self._persist, protocol = _pickle.HIGHEST_PROTOCOL)
        self._persist.flush()
        return True


class MultiPicker(PopupPlugin, FancyItemsMixin):

    SIMPLE_SECTION_DEFAULT = "value"
    OPTIONS = {
        "tie:history": "history"
    }

    def bootstrap(self):
        PopupPlugin.bootstrap(self)
        FancyItemsMixin.bootstrap(self)
        self._types = (
            ("value", self._value),
            ("callable", self._callable),
            ("action", self._action),
            ("notify", self._notify),
        )

    def items(self):
        for label, options in FancyItemsMixin.items(self):
            callable = lambda: self.history.top

            for type, handler in self._types:
                if type in options:
                    callable = handler(label, options, callable)

            yield (label, callable)

    def _value(self, label, options, callable):
        return _ft.partial(options.get, "value")

    def _callable(self, label, options, callable):
        try:
            return _util.load_dotted(options["callable"])(options, self)
        except HistoryEmpty:
            _log.info("History is empty")
            self.klemmbrett.notify("The history is empty", "The history is empty")


    def _action(self, label, options, callable):
        try:
            return _ft.partial(self._perform_action, options, callable)
        except HistoryEmpty:
            _log.info("History is empty")
            self.klemmbrett.notify("The history is empty", "The history is empty")

    def _notify(self, label, options, callable):
        return _ft.partial(self.klemmbrett.notify, "Klemmbrett", options.get("notify"))

    @staticmethod
    def _perform_action(options, callable):
        _gobject.spawn_async([
            "/bin/bash",
            "-c",
            options["action"] % (callable(),),
        ]),

    def set(self, widget = None, text = None):
        return PopupPlugin.set(self, widget = widget, text = text)


class ActionPicker(MultiPicker):
    DEFAULT_BINDING = "<Ctrl><Alt>A"
    SIMPLE_SECTION = "actions"
    COMPLEX_SECTION_PREFIX= "action "
    SIMPLE_SECTION_DEFAULT = "action"


class SnippetPicker(MultiPicker):
    DEFAULT_BINDING = "<Ctrl><Alt>S"
    SIMPLE_SECTION = "snippets"
    COMPLEX_SECTION_PREFIX= "snippet "
