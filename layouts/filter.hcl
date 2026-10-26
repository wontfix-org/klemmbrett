location = "center"

window {
  children = ["inputbar", "listview"]
}

inputbar {
  children = ["prompt", "entry"]
  padding  = 8
  spacing  = 8
}

element {
  children = ["element-marker", "element-index", "element-text"]
}

element-marker {
  enabled = widget.multi
}
