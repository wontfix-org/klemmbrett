# Dracula
# https://draculatheme.com
#
# background #282a36  current-line/selection #44475a
# foreground #f8f8f2  comment #6272a4

defaults {
  background_color = hex("282a36")
  text_color       = hex("f8f8f2")
  border_color     = hex("44475a")
}

overlay {
  background_color = transparent()
}

window {
  border        = 1
  border_radius = 4
}

element "selected" {
  background_color = hex("44475a")
}

element "hover" {
  background_color = hex("44475a")
}

element-index {
  text_color = hex("6272a4")
}
