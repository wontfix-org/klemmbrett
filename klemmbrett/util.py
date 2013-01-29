#!/usr/bin/env python

from distutils import util as _util


def humanbool(value):
    return _util.strtobool(str(value).strip().lower() or 'no')


def load_dotted(name):
    """ stolen from andre malos wtf daemon """
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


