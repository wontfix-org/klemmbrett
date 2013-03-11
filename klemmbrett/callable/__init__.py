
def newline_to_comma(options, plugin):
    return lambda: plugin.history.top.strip().replace("\n", ",")
