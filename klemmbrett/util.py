from distutils import util as _util

def humanbool(value):
    return _util.strtobool(str(value).strip().lower() or 'no')

