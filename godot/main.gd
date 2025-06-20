extends Node2D


# Called when the node enters the scene tree for the first time.
func _ready() -> void:
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta: float) -> void:
	$ClientButton/Label.text = "Amount: " + str($ClientButton.remote_player_amount) + " Local Player Id: " + $ClientButton.get_local_player_id()




func _on_server_button_pressed() -> void:
	$ServerButton/Label.text = "Is Running"
