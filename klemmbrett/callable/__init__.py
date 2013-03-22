import re as _re

def newline_to_comma(options, plugin):
    return plugin.history.top.strip().replace("\n", ",")

def newline_to_comma_quoted(options, plugin):
    return "'" + "','".join(_re.split(r"\s+", plugin.history.top.strip())) + "'"
