
def newline_to_comma(options, plugin):
    return plugin.history.top.strip().replace("\n", ",")
