# mbra — qtile focus, tmux status, and Alacritty
#
# focus   #60845b  unfocused #50594b  current #0f0f0f
# pane    #B9B9B9  terminal  #000000  text    #e5e5e5

defaults {
  background_color = hex("000000")
  text_color       = hex("e5e5e5")
  border_color     = hex("50594b")
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
  background_color = hex("60845b")
  text_color       = hex("0f0f0f")
}

element "hover" {
  background_color = hex("50594b")
}

element-index {
  text_color = hex("B9B9B9")
}

inputbar {
  background_color = hex("0f0f0f")
  border_radius    = 10
  padding          = [8, 12]
  height           = 40
}

prompt {
  text_color = hex("60845b")
}
