# Tokyo Night
# https://github.com/tokyo-night/tokyo-night-vscode-theme
#
# bg      #1a1b26  highlight #292e42  fg       #c0caf5
# comment #565f89  blue      #7aa2f7  surface  #16161e

defaults {
  background_color = hex("1a1b26")
  text_color       = hex("c0caf5")
  border_color     = hex("292e42")
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
  background_color = hex("3d59a1")
}

element "hover" {
  background_color = hex("292e42")
}

element-index {
  text_color = hex("565f89")
}

inputbar {
  background_color = hex("16161e")
  border_radius    = 10
  padding          = [8, 12]
  height           = 40
}

prompt {
  text_color = hex("7aa2f7")
}
