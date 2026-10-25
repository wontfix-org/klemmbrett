# Nord — arctic, north-bluish palette
# https://www.nordtheme.com/docs/colors-and-palettes
#
# nord0 #2e3440  nord1 #3b4252  nord2 #434c5e  nord3 #4c566a
# nord4 #d8dee9  nord6 #eceff4  nord10 #5e81ac

defaults {
  background_color = hex("2e3440")
  text_color       = hex("eceff4")
  border_color     = hex("3b4252")
}

overlay {
  background_color = transparent()
}

window {
  border        = 1
  border_radius = 4
}

element "selected" {
  background_color = hex("434c5e")
}

element "hover" {
  background_color = hex("3b4252")
}

element-index {
  text_color = hex("4c566a")
}
