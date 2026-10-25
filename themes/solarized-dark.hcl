# Solarized Dark — Ethan Schoonover
# https://ethanschoonover.com/solarized/
#
# base03 #002b36  base02 #073642  base01 #586e75  base0 #839496
# base1  #93a1a1  blue   #268bd2

defaults {
  background_color = hex("002b36")
  text_color       = hex("839496")
  border_color     = hex("073642")
}

overlay {
  background_color = transparent()
}

window {
  border        = 1
  border_radius = 0
}

element "selected" {
  background_color = hex("073642")
  text_color       = hex("93a1a1")
}

element "hover" {
  background_color = hex("073642")
}

element-index {
  text_color = hex("586e75")
}
