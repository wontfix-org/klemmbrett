#!/usr/bin/env python

import os as _os
import functools as _ft
import weakref as _weakref
import collections as _collections
import cPickle as _pickle

import pygtk as _pygtk
_pygtk.require('2.0')
import gtk as _gtk
import gobject as _gobject
import keybinder as _keybinder

import klemmbrett.util as _util
import klemmbrett as _klemmbrett
import klemmbrett.config as _config


class Plugin(_gobject.GObject):

    OPTIONS = {}

    def __init__(self, name, options, klemmbrett):
        super(Plugin, self).__init__()
        self.klemmbrett = _weakref.proxy(klemmbrett)
        self.options = options
        self.name = name

    def set(self, widget = None, text = None):
        #print "setting new content: %r" % (buf,)
        if callable(text):
            text = text()
        self.klemmbrett.set(text)

    def _printable(self, text, htmlsafe = False):
        clean = text.replace('\n', ' ')[

            :min(
                len(text),
                int(self.options.get('line-length', 30)),
            )
        ].strip()

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
        self.tray.set_tooltip("Klemmbrett")
        self.tray.connect('popup-menu', self.on_menu, self.menu)

        icon = self.options.get('icon-path', None)
        if icon:
            self.tray.set_from_file(_os.path.expanduser(icon))
        else:
            self.tray.set_from_stock(_gtk.STOCK_ABOUT)

    def on_menu(self, icon, event_button, event_time, menu):
        menu.popup(
            None,
            None,
            _gtk.status_icon_position_menu,
            event_button,
            event_time,
            self.tray,
        )


class PopupPlugin(Plugin):

    def bootstrap(self):
        _keybinder.bind(
            self.options.get('shortcut', self.DEFAULT_BINDING),
            self.popup,
        )

    def popup(self):
        menu = _gtk.Menu()
        index = 0

        # XXX(mbra): this will not work with values as "no", "off" etc.
        # since we do not use getbool
        if _util.humanbool(self.options.get('show-current-selection', 'yes')) and len(self.history):
            item = _gtk.MenuItem("")
            item.get_children()[0].set_markup(
                "<b>%s</b>" % (self._printable(self.history.top, True),),
            )
            menu.append(item)
            menu.append(_gtk.SeparatorMenuItem())
            index += 1

        for pos, (label, value) in enumerate(self.items()):
            label = "_%s %s" % (pos, label)
            item = _gtk.MenuItem(label, use_underline = True)
            item.connect("activate", self.set, value)
            menu.append(item)

        menu.show_all()
        menu.popup(
            None,
            None,
            None,
            1,
            _gtk.get_current_event_time(),
        )
        menu.set_active(index)
        return True


class FancyItemsMixin(object):

    def items(self):
        section = self.options.get('simple-section', self.SIMPLE_SECTION)

        if not self.klemmbrett.config.has_section(section):
            raise KeyError("No config section %s defined" % (section,))

        for item in self.klemmbrett.config.items(section):
            yield item


        prefix = self.options.get('complex-section-prefix', self.COMPLEX_SECTION_PREFIX)
        if not prefix:
            raise StopIteration()

        for section in self.klemmbrett.config.sections():
            if not section.startswith(prefix):
                continue

            options = dict(self.klemmbrett.config.items(section))
            label = section[len(prefix):]

            if "callable" in options:
                yield (
                    label,
                    _ft.partial(
                        _util.load_dotted(options["callable"]),
                        options = options,
                        plugin = self,
                    ),
                )
                continue

            if not "value" in options:
                raise KeyError(
                    "No value found for %r in section %s" % (
                        self.__class__.__name__,
                        section,
                    )
                )

            yield (label, options["value"])


class HistoryPicker(PopupPlugin):

    DEFAULT_BINDING = "<Ctrl><Alt>C"

    __gsignals__ = {
        "text-accepted": (_gobject.SIGNAL_RUN_FIRST, None, (_gobject.TYPE_PYOBJECT,)),
    }

    OPTIONS = {
        "tie:history": "history",
    }

    def __init__(self, *args, **kwargs):
        super(HistoryPicker, self).__init__(*args, **kwargs)

        self._history = _collections.deque(
            maxlen = self.maxlen,
        )
        self._extend_detection = _util.humanbool(self.options.get('extend-detection', 'yes'))

    def bootstrap(self):
        super(HistoryPicker, self).bootstrap()
        self.klemmbrett.connect("text-selected", self._text_selected)

    def items(self):
        for text in self._history:
            yield (
                self._printable(text),
                text,
            )

    def add(self, text, emit = True):
        if self.accepts(text):
            if (
                self._extend_detection
                and len(self)
                and text.startswith(self.top)
            ):
                self._history[0] = text
            else:
                self._history.appendleft(text)

            if emit:
                self.emit("text-accepted", text)
            return True
        return False

    def _text_selected(self, widget, text):
        return self.add(text)

    def __iter__(self):
        return iter(self._history)

    def __len__(self):
        return len(self._history)

    def accepts(self, text):
        # do not accept bullshit
        if not isinstance(text, basestring):
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
        return self._history[0]

    @property
    def maxlen(self):
        return int(self.options.get("length", 15))


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


class SnippetPicker(PopupPlugin, FancyItemsMixin):

    DEFAULT_BINDING = "<Ctrl><Alt>S"
    SIMPLE_SECTION = "snippets"
    COMPLEX_SECTION_PREFIX= "snippet "

    OPTIONS = {
        "tie:history": "history"
    }


class ActionPicker(PopupPlugin, FancyItemsMixin):

    DEFAULT_BINDING = "<Ctrl><Alt>A"
    SIMPLE_SECTION = "actions"
    COMPLEX_SECTION_PREFIX= "action "

    OPTIONS = {
        "tie:history": "history"
    }

    def set(self, widget = None, text = None):
        try:
            command = "/bin/bash -c " + text % (self.history.top,)
            _gobject.spawn_async(["/bin/bash", "-c", text % (self.history.top,)])
        except StopIteration:
            pass
