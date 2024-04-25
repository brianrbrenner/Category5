#version 450
#extension GL_ARB_separate_shader_objects : enable

layout(location = 0) in vec2 loc;
layout(location = 1) in vec2 coord;

layout(location = 0) out vec2 fragcoord;

layout(binding = 0) uniform ShaderConstants {
 mat4 model;
 float width;
 float height;
} ubo;

layout(push_constant) uniform PushConstants {
 vec2 viewport_offset;
 float width;
 float height;
 float starting_depth;
 // The id of the image. This is the offset into the unbounded sampler array.
 // id that's the offset into the unbound sampler array
 int image_id;
 // if we should use color instead of texturing
 int use_color;
 // Padding to match our shader's struct
 int padding;
 vec4 color;
 // The complete dimensions of the window.
 vec2 surface_pos;
 vec2 surface_size;
} push;

/* The array of textures that are the window contents */
layout(set = 1, binding = 1) uniform sampler2D images[];

void main() {
 // Add our viewport offset to the location
 vec2 position = push.surface_pos + push.viewport_offset;

 // 1. loc should ALWAYS be 0,1 for the default quad.
 // 2. multiply by two since the axis are over the range (-1,1).
 // 3. multiply by the percentage of the screen that the window
 //    should take up. Any 1's in loc will be scaled by this amount.
 // 4. add the (x,y) offset for the window.
 // 5. also multiply the base by 2 for the same reason
 //
 // Use viewport size here instead of the total resolution size. We want
 // to scale around our display area, not the entire thing.
 vec2 adjusted = loc
  * vec2(2, 2)
  * (push.surface_size / vec2(push.width, push.height))
  + (position / vec2(push.width, push.height))
  * vec2(2, 2);

 // use our instance number as the depth. Smaller means farther back in
 // the scene, so we are drawing back to front but our depth value is
 // increasing
 //
 // We also have a starting depth that is set, which keeps track of the
 // latest depth to start at. This will be updated every time a surface
 // list is drawn in thundr, that way lists don't Z-fight
 // One hundred million objects is our max right now.
 gl_Position = ubo.model * vec4(adjusted, push.starting_depth, 1.0);

 fragcoord = coord;
}
