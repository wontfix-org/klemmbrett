#klemmbrett

## Getting started

### GIT

Clone the GIT Repository into a directory of your liking.

Klemmbrett by default uses Ctrl-Alt-{C,A,S} as shortcuts, your desktop environment may use
them for something else or you may not like them, so you should either modify klemmbrett.conf
to suit your needs or unbind them in your desktop environment.

```
cp conf/klemmbrett.conf $HOME/.klemmbrett.conf
PYTHONPATH="." scripts/klemmbrett
```

 * Select some text, hit Ctrl-Alt-C to see your clipboard history

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
[snippet sql]
callable = klemmbrett.callable.alchemy.statement
engine = postgres://user:pw@host/database
statement = select id from products limit 1
```
