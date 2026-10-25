# Catppuccin Mocha
# https://github.com/catppuccin/palette
#
# base    #1e1e2e  surface0 #313244  surface1 #45475a
# text    #cdd6f4  overlay0 #6c7086  mauve    #cba6f7

defaults {
  background_color = hex("1e1e2e")
  text_color       = hex("cdd6f4")
  border_color     = hex("313244")
}

overlay {
  background_color = transparent()
}

window {
  border        = 1
  border_radius = 8
}

element "selected" {
  background_color = hex("45475a")
}

element "hover" {
  background_color = hex("313244")
}

element-index {
  text_color = hex("6c7086")
}
