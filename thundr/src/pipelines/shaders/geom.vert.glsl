#version 450
#extension GL_ARB_separate_shader_objects : enable
#extension GL_EXT_nonuniform_qualifier : enable

layout(location = 0) in vec2 loc;
layout(location = 1) in vec2 coord;

layout(location = 0) out vec2 fragcoord;
layout(location = 1) flat out int window_index;

layout(binding = 0) uniform ShaderConstants {
mat4 model;
float width;
float height;
} ubo;

struct Rect {
	ivec2 start;
	ivec2 size;
};

struct Window {
	/* id.0 is the id. It is an ivec4 for alignment purposes */
	/* id.0: id that's the offset into the unbound sampler array */
	/* id.1: if we should use w_color instead of texturing */
	ivec4 id;
	/* the color used instead of texturing */
	vec4 color;
	Rect dims;
	Rect opaque;
};

/* the position/size/damage of our windows */
layout(set = 1, binding = 0, std140) buffer window_list
{
	layout(offset = 0) int total_window_count;
	layout(offset = 16) Window windows[];
};

layout(set = 1, binding = 1, std140) buffer order_list
{
	layout(offset = 0) int window_count;
	layout(offset = 16) int ordered_windows[];
};


/* The array of textures that are the window contents */
layout(set = 1, binding = 2) uniform sampler2D images[];

void main() {
	// Go in reverse order, so that alpha works correctly
	int index = (window_count - 1) - gl_InstanceIndex;
	window_index = ordered_windows[index];

	// 1. loc should ALWAYS be 0,1 for the default quad.
	// 2. multiply by two since the axis are over the range (-1,1).
	// 3. multiply by the percentage of the screen that the window
	//    should take up. Any 1's in loc will be scaled by this amount.
	// 4. add the (x,y) offset for the window.
	// 5. also multiply the base by 2 for the same reason
	vec2 adjusted = loc
		* vec2(2, 2)
		* (windows[window_index].dims.size / vec2(ubo.width, ubo.height))
		+ (windows[window_index].dims.start / vec2(ubo.width, ubo.height))
		* vec2(2, 2);

	// The model transform will align x,y = (0, 0) with the top left of
	// the screen. It should stubtract 1.0 from x and y.
	float order = 0.0 + (float(window_index) * 0.0000001);
	gl_Position = ubo.model * vec4(adjusted, order, 1.0);

	fragcoord = coord;
}
