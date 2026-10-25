# Gruvbox Dark (medium contrast) — morhetz
# https://github.com/morhetz/gruvbox
#
# dark0 #282828  dark1 #3c3836  dark2 #504945
# fg    #ebdbb2  gray  #928374

defaults {
  background_color = hex("282828")
  text_color       = hex("ebdbb2")
  border_color     = hex("504945")
}

overlay {
  background_color = transparent()
}

window {
  border        = 1
  border_radius = 2
}

element "selected" {
  background_color = hex("3c3836")
}

element "hover" {
  background_color = hex("3c3836")
}

element-index {
  text_color = hex("928374")
}
