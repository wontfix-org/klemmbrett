# Everforest dark, medium contrast
# https://github.com/sainnhe/everforest
#
# bg0   #2d353b  bg1  #343f44  bg2   #3d484d
# fg    #d3c6aa  grey #859289  green #a7c080
# aqua  #83c092  blue #7fbbb3

defaults {
  background_color = hex("2d353b")
  text_color       = hex("d3c6aa")
  border_color     = hex("3d484d")
}

overlay {
  background_color = transparent()
}

window {
  border        = 1
  border_radius = 14
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
  background_color = hex("475258")
  text_color       = hex("d3c6aa")
}

element "hover" {
  background_color = hex("3d484d")
}

element-index {
  text_color = hex("859289")
}

inputbar {
  background_color = hex("343f44")
  border_radius    = 10
  padding          = [8, 12]
  height           = 40
}

prompt {
  text_color = hex("a7c080")
}
