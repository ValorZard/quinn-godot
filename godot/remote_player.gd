extends Player


# Called when the node enters the scene tree for the first time.
func _ready() -> void:
	print("player puppet spawned")
	pass # Replace with function body.


# Called every frame. 'delta' is the elapsed time since the previous frame.
func _process(delta: float) -> void:
	$Label.text = get_player_id()
