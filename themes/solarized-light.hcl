# Solarized Light — Ethan Schoonover
# https://ethanschoonover.com/solarized/
#
# base3 #fdf6e3  base2 #eee8d5  base1 #93a1a1  base00 #657b83
# base01 #586e75

defaults {
  background_color = hex("fdf6e3")
  text_color       = hex("657b83")
  border_color     = hex("eee8d5")
}

overlay {
  background_color = transparent()
}

window {
  border        = 1
  border_radius = 0
}

element "selected" {
  background_color = hex("eee8d5")
  text_color       = hex("586e75")
}

element "hover" {
  background_color = hex("eee8d5")
}

element-index {
  text_color = hex("93a1a1")
}
