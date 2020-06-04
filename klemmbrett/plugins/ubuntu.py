# coding: utf-8
import os as _os

import gi as _gi
_gi.require_version('Gtk', '3.0')
from gi.repository import Gtk as _gtk
_gi.require_version('AppIndicator3', '0.1')
from gi.repository import AppIndicator3 as _appindicator
from klemmbrett import plugins as _plugins

import klemmbrett.util as _util
import klemmbrett.about as _about


class AppIndicatorPlugin(_plugins.Plugin):

    def __init__(self, *args, **kwargs):
        super(AppIndicatorPlugin, self).__init__(*args, **kwargs)

        icon = self.options.get("icon-path", _util.get_status_icon_filename())
        self.indicator = _appindicator.Indicator.new(
            "klemmbrett",
            icon,
            _appindicator.IndicatorCategory.APPLICATION_STATUS,
        )
        self.indicator.set_icon_full(icon, "Klemmbrett")
        self.indicator.set_status(_appindicator.IndicatorStatus.ACTIVE)

        self.menu = _gtk.Menu()

        item = _gtk.MenuItem("Quit")
        item.connect('activate', _gtk.main_quit)
        self.menu.append(item)

        item = _gtk.MenuItem("About")
        item.connect('activate', _about.about)
        self.menu.append(item)

        self.menu.show_all()

        self.indicator.set_menu(self.menu)
