# klemmbrett

## Getting started

Besides just installing klembrett via pip, you will need to make sure a couple system packages are installed.
We use keybinder for global shortcuts and gtk3 for clipboard handling and the popup menus, so make sure the
packages for your distro are installed, also make sure the typelib files for gobject introspection are installed
if your distribution ships them as seperate packages (as Debian/Ubuntu does).


```
apt-get install libgirepository1.0-dev gir1.2-keybinder-3.0 gir1.2-gtk-3.0 gir1.2-glib-2.0
pip install klemmbrett
```

If you do not like to install pip packages into your system globally, either use a virtualenv or `pip install --user`.

## Adding your own stuff

### Actions

Actions trigger the execution of a commandline with the clipboard contents injected at a user specified position.
To configure a simple action, find the *actions* section in the config file and add a new ``label = command`` entry.
To inject the current clipboard contents, use ``%s`` anywhere in the commandline string.

```
search = firefox "http://www.duckduckgo.com/?q=%s"
```

### Snippets

## Static Snippets

Although snippets also have the ability to product a side-effect by executing python code, it's main goal is to
produce content that is place in the clipboard for you to use. The simplest form is static content. This is done
the same way an action is configured, just add a ``label = text`` pair to the *snippets* and the snippet should
be available to you when you hit the snippets shortcut.

## Dynamic Snippets

Static snippets are boring, what makes the snippets really useful is the possibility to run a python callable
that uses the current clipboard contents to generate new clipboard contents. To tell Klemmbrett to do this,
just prefix the label with *callable.* and make the text part the global path to a python callable in dotted
notation.

```
callable.n2c = klemmbrett.callable.newline_to_comma
```

This will add a stock callable snippet to your snippets menu, that converts newline sequences to commas.
This can for example be used to vertically select a column of an sql query output in the mysql oder psql
commandline clients, and convert it to content suitable for reuse in an IN() condition.

## Advanced dynamic snippets

Sometimes you want to configure more than just a simple dynamic snippet, perhaps you want to parameterize
your calls for different situations so you do not copy, paste & modify your code too often.

To do that, instead of adding a new key to the snippets section, you add a new section named `snippet <label>`.
In that section you can now add an arbitrary number of keys that get passed to the callable in addition to
the reserved keys that are required to setup the snippet.

```
[snippet random product]
callable = klemmbrett.callable.alchemy.statement
engine = postgres://user:pw@host/database
statement = select id from products limit 1
```

Additionally, the statement plugin passes the current clipboard contents to the prepared statement as :0,
so you may do a lookup based on it and return what has been found.

```
[snippet lookup product name]
callable = klemmbrett.callable.alchemy.statement
engine = postgres://user:pw@host/database
statement = select name from products where id = :0 limit 1
```
