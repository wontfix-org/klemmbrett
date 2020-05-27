#!/usr/bin/env python

import html as _html
import distutils.util as _util

try:
    from compiler.consts import CO_GENERATOR
except ImportError:
    # IronPython doesn't have a complier module
    CO_GENERATOR = 0x20


def isgenerator(func):
    """ Check if a given function is a generator before calling it """
    try:
        return func.func_code.co_flags & CO_GENERATOR != 0
    except AttributeError:
        return False


def yieldwrap(func, *args, **kwargs):
    """ Ensure that the toplevel function is seen as a generator """
    def wrapped():
        for i in func(*args, **kwargs):
            yield i
    return wrapped


def humanbool(value):
    """
        Use distutils.util strtobool to convert various boolean tokens
        like yes, on, no, off etc to a boolean
    """
    return _util.strtobool(str(value).strip().lower() or 'no')


def load_dotted(name):
    """ Imports and return the given dot-notated python object """
    components = name.split('.')
    path = [components.pop(0)]
    obj = __import__(path[0])
    while components:
        comp = components.pop(0)
        path.append(comp)
        try:
            obj = getattr(obj, comp)
        except AttributeError:
            __import__('.'.join(path))
            try:
                obj = getattr(obj, comp)
            except AttributeError:
                raise ImportError('.'.join(path))
    return obj


def htmlsafe(text):
    """ Escape htmlentities """
    return _html.escape(text)

