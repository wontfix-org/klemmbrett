# coding: utf-8

import gi as _gi
_gi.require_version('Gtk', '3.0')
from gi.repository import Gtk as _gtk
from gi.repository import GdkPixbuf as _pixbuf


import klemmbrett.util as _util

def about(event):
    dialog = _gtk.AboutDialog()
    # lists of authors and documenters (will be used later)
    authors = [
        "Michael van Bracht",
        "Roland Sommer",
        "Sven Wegener",
        "Marc Schmitzer",
    ]

    # we fill in the aboutdialog
    dialog.set_program_name("Klemmbrett")
    dialog.set_authors(authors)
    dialog.set_artists(["Tray icon provided by icons8.com https://icons8.com"])
    dialog.set_website("https://github.com/wontfix-org/klemmbrett")
    dialog.set_copyright("Tray Icon provided by https://icons8.com")
    dialog.set_website_label("Klemmbrett on GitHub")
    dialog.set_license_type(_gtk.License.MIT_X11)
    logo = _pixbuf.Pixbuf.new_from_file(_util.get_status_icon_filename())
    dialog.set_logo(logo)

    # the program name
    dialog.set_title("")

    # to close the aboutdialog when "close" is clicked we connect the
    # "response" signal to on_close
    # show the aboutdialog
    dialog.show()
