# Kanagawa wave
# https://github.com/rebelot/kanagawa.nvim
#
# sumiInk1 #1f1f28  sumiInk3 #363646  fujiWhite #dcd7ba
# fujiGray #727169  waveBlue #2d4f67  crystal   #7e9cd8

defaults {
  background_color = hex("1f1f28")
  text_color       = hex("dcd7ba")
  border_color     = hex("363646")
}

overlay {
  background_color = transparent()
}

window {
  border        = 1
  border_radius = 16
  padding       = 10
  spacing       = 8
}

listview {
  spacing = 4
}

element {
  padding       = [4, 10]
  border_radius = 8
  height        = 32
}

element "selected" {
  background_color = hex("2d4f67")
}

element "hover" {
  background_color = hex("2a2a37")
}

element-index {
  text_color = hex("727169")
}

inputbar {
  background_color = hex("16161d")
  border_radius    = 10
  padding          = [8, 12]
  height           = 40
}

prompt {
  text_color = hex("7e9cd8")
}
