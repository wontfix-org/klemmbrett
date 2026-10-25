# Rosé Pine
# https://rosepinetheme.com
#
# base    #191724  surface #1f1d2e  overlay #26233a
# text    #e0def4  muted   #6e6a86  iris    #c4a7e7
# foam    #9ccfd8  rose    #ebbcba

defaults {
  background_color = hex("191724")
  text_color       = hex("e0def4")
  border_color     = hex("26233a")
}

overlay {
  background_color = transparent()
}

window {
  border        = 1
  border_radius = 18
  padding       = 12
  spacing       = 8
}

listview {
  spacing = 4
}

element {
  padding       = [4, 12]
  border_radius = 10
  height        = 32
}

element "selected" {
  background_color = hex("26233a")
  text_color       = hex("e0def4")
  border           = 1
  border_color     = hex("c4a7e7")
}

element "hover" {
  background_color = hex("1f1d2e")
}

element-index {
  text_color = hex("6e6a86")
}

inputbar {
  background_color = hex("1f1d2e")
  border_radius    = 12
  padding          = [8, 12]
  height           = 40
}

prompt {
  text_color = hex("ebbcba")
}
