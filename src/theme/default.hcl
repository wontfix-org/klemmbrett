# Default klemmbrett popup theme. Matches the built-in GPUI look.

defaults {
  background_color = hex("2b2b2b")
  text_color       = hex("e8e8e8")
  border_color     = hex("555555")
}

overlay {
  background_color = transparent()
}

window {
  border       = 1
  border_radius = 0
  padding      = 0
  font_size    = 14
  max_height   = 600
  orientation  = "vertical"
  children     = ["listview"]
}

listview {
  spacing = 0
}

element {
  orientation = "horizontal"
  children    = ["element-index", "element-text"]
  padding     = [0, 8]
  spacing     = 8
  height      = 28
  background_color = transparent()
}

element "selected" {
  background_color = hex("3d5a80")
}

element "hover" {
  background_color = hex("3d5a80")
}

element-index {
  text_color       = hex("888888")
  width            = 16
  background_color = transparent()
}

element-text {
  background_color = transparent()
}
