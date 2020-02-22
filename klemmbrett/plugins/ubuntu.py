# coding: utf-8
import os as _os
import appindicator as _appindicator

import gi as _gi
_gi.require_version('Gtk', '3.0')
from gi.repository import Gtk as _gtk
from klemmbrett import plugins as _plugins

class AppIndicatorPlugin(_plugins.Plugin):

    def __init__(self, *args, **kwargs):
        super(AppIndicatorPlugin, self).__init__(*args, **kwargs)

        self.indicator = _appindicator.Indicator(
            "klemmbrett",
            "indicator-messages",
            _appindicator.CATEGORY_APPLICATION_STATUS
        )

        icon = self.options.get('icon-path', None)

        self.indicator.set_icon(_os.path.expanduser(icon))

        self.menu = _gtk.Menu()
        item = _gtk.MenuItem("Quit")
        item.connect('activate', _gtk.main_quit)
        self.menu.append(item)
        self.menu.show_all()

        self.indicator.set_menu(self.menu)
        self.indicator.set_status(_appindicator.STATUS_ACTIVE)


