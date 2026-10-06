# Star Citizen: CitizenCon 2949 - Terra Firmer
https://www.youtube.com/watch?v=IfCc_aDNsAw
Audio duration: 00:44:06
Source: Whisper large-v3 (unquantized), whisper.cpp, Vulkan / RX 9070 XT, English, beam size 5.
Technical spelling corrections, where applied, are documented in term-corrections.json and correction-audit.json.
This is a full speech transcript, not a summary. Machine transcription can still contain errors.

## 00:00:00 — Introductions

**[00:00:00]** Hi everyone, my name is Marco Corbetta, Senior Technical Director. My name is Michel Kooper, Lead Environment Artist on the organics team. And I'm Ali Brown, Director of Graphics Engineering. And we're going to talk about the evolution of planet tech today

## 00:00:31 — Planet tech V1–V3: seamless planets and ecosystems — Marco Corbetta

**[00:00:31]** initially our goal was to achieve a seamless transition between space and planets so fly to a distant planet land on it and explore its entire surface with no loading screens so looking up at the planets in the sky and fly up to them is cool but what are the planets supposed to look like and how can we create them. So initially, I just started doing some tests mixing procedural terrain with buildings. Here is another early picture showing procedural terrain with no mountains.

**[00:01:08]** While we were getting some very interesting results during our early test, a key factor in Star Citizen is that we have a specific universe with pretty deep lore. So we cannot just generate, you know, random planets. We needed artist input to match the reference and lore. So we had multiple layers of noise and some generic elevation maps. Shading and colors were based on procedural rules, elevation and slope. We had an initial editor integration

**[00:01:40]** with real time editing of basic planet properties and atmosphere. So after a lot of hard work, we got our first version V1 working. Here you can see our first man on the planet. And here is the first spaceship on the planet. Successful landing. Excellent. So after V1, we introduced procedural and artistic improvements.

**[00:02:13]** During V2, we introduced the concept of ecosystem. So the ecosystems are a combination of terrain maps with visual objects and gameplay properties. Each ecosystem had three-channel color textures used for blending texture layers, and we used the same channels for object scattering as well. Using ecosystems, we can make each planet unique, matching the lore, but still generating planet-sized content for players to explore.

**[00:02:48]** So V2 shipped with Alpha 3.0, we had the new moons and landing zones, we had seamless transitions, details visible from space all the way down to the surface. We basically rewrote the entire 3D engine terrain system to work at scale and on a spherical planet. Alpha 3.0 was multiplayer ready. You could already explore planets online with your friends.

**[00:03:19]** You could go to flight formations and planet side battles. The planet generation is synchronized between client and server. The physics collision mesh is generated on demand, and we will talk a bit more about this later. This picture is showing the sense of scale we achieved with Alpha 3.0. As you can see here, we are zooming in from a solar system down to a planet's surface. Here is Daymar,

**[00:03:56]** planet view from space. While zooming in, we start seeing high-level formations and ecosystems getting closer objects are starting to appear next and here is the space the ground-to-space transition for Yela showing additional elements like an icy surface from space. Next slide, please. Okay. And this is the other way around, zooming

**[00:04:39]** out from ground to space. Okay, with version 3 we decided to introduce more artistic control. Here we can see the artistic improvements made to Yela in version Version 3. Version 3 shipped with Alpha 3.3.5. We added new moons and new planets, including

**[00:05:14]** a city planet cover with buildings. We introduced real day/night cycles, so planets are actually rotating in real time. We made a large amount of tools improvements and built a new vegetation

## 00:05:26 — V3 texture layers and temporal dithering

**[00:05:26]** system. In V3, we increased from three channels to 16 channels using alpha values to encode the material IDs. But blending these 16 texture layers for each pixel would be way too expensive, so we found a very fast solution by blending only the closest surface using temporal dithering. So here we have several layers combined with each other, but without smooth blending.

**[00:05:59]** And here we're blending 60 [possibly 16] layers at once using temporal dithering. So it's basically at the cost of blending only one layer. So what would be the next steps after version 3? We had the desire for something quicker to generate with less direct artistic control like we had previously in V2, but still being able to influence terrain shape and colours like in V3. We want a smaller ecosystem to get more details, improve blending and transitions,

**[00:06:30]** but still we need to take into account special planets like ArcCorp. So enter planets V4, our final version. We decided to go for a more physically based approach. We wanted to improve on ecosystems blending and transitions, and now I'm handing

## 00:06:50 — Why V4: biome variety and artist workflow — Michel Kooper

**[00:06:50]** over to Michel to give us an overview of V4. Thank you, Marco. I hope this clicker holds up because there seems to be some interference. So, like I said, I'm the lead artist on the organics team. Our team works with the planet tech tool day in, day out. All the planets you've seen so far have been created by the team in DE. So, we had some thoughts and demands whenever we talk about tools and what we want to improve. So, why v4? Let's go over some

**[00:07:23]** of this stuff. So, why v4? Up until now, for a long time, our focus has been on moon landscapes. And although these are visually distinct from each other, they're essentially a single biome planetoid. One is desert, one is icy, one is a lunar landscape, but the biggest difference is actually the type of assets that you see, the shapes in the terrain. Last year, when the team started to work on Hurston, we were actually confronted with our biggest challenge yet. Hurston is a very diverse planet in terms of what we've done

**[00:07:57]** so far. So we have dry wastelands, we had our mining pits, we have trash yards, we have hot acidic areas, and even very lush green areas. So on a single planet, we had to cover a lot of variety. And this clearly proved a challenge. So based on this experience working on Hurston, we started to think about, okay, what can we do to improve where are our bottlenecks, and how can we proceed? So we wanted to make sure that we future-proof

**[00:08:28]** the technology, because up until now, it's been Stanton. But there is -- we want to go somewhere, right? We want to go to other locations. And the only way to do it is to work faster and more efficient. So V4 is a fundamental change in how fast we can build planets. Keeping what worked well, reworking the things that slowed us down, and building a tool that's in engine and is a very intuitive, artist-friendly tool to use. And we'll show some examples of that coming up.

**[00:09:01]** Yes. All right. Cool. Seems to work. So here's an example of some of the stuff that we wanted to achieve or at least wanted to do, but we felt we couldn't really do. So you see here a Google image of the African continent. And what you can clearly see is on a large scale the transition from a desert landscape to a savanna to a thick jungle. And I don't know know if any of you ever enjoyed to look around on Google Earth. I personally like to do that

**[00:09:33]** a lot, reference gathering, et cetera. And I always enjoyed seeing these beautiful transitions. And they seem quite strong, but if you go up close, you can actually tell that these transitions are, like, incredibly complex. So, there's all these features in the terrain that inform why these transitions happen. In this case, where the vegetation gets more lush, you see that the mountainscapes start to block the drier desert winds. The water paths are the first areas

**[00:10:05]** where trees start to grow. And when you zoom into these areas, you can actually see how the terrain and how the terrain features actually start to inform these transitions. Another example, I just have a few examples to go through. Another one where the terrain creates these beautiful patterns of transitions in between different biome types. And lastly, wow. Please. There we go. European Alps where the transition

**[00:10:36]** in height actually causes a transition from like a regular green area to like the snowy mountains. And these transitions happen over an incredibly large area. So, let's have a look and go back to Hurston. And have a bit of a chat about the challenges we had. So, we were not really unhappy with the results that we were getting. Especially up close. I think that was pretty good. Especially considering the scale of our game. But we needed a wider

**[00:11:07]** range of biomes, and we could only imagine, like, if you think about Terra Prime or anything that's very complex that we still want to do, we needed a system that worked smarter and better. So it became clear that our tiled approach was making it really hard to scale and create convincing transitions. Next slide. All right. So this is a very unflattering a look of Hurston. I turned off the atmosphere and some of the effects that usually go on

**[00:11:39]** top of this. But what we usually do, or did before, was paint a global texture that sort of represents the different areas on a planet. So what we have here is on the pole. It's a bit desaturated, but that's where the savannah area meets the wasteland. And although it's not super crisp, it kind of gets the point across, especially with the effects layered on top of it. When you start flying close to the surface, especially with all the effects turned off, you start to see where the global texture starts to fill, and you get this blurry

**[00:12:11]** look, the textures that are loaded locally for each patch of terrain, or each tile is not fully loaded in, and you hit this dead zone in between where it just looks a bit blurry, a bit muddy, and it's not really getting us the results that we saw on Google Earth. And then when you get even closer, you start to see the individual patches coming in. the texture of the biome becomes visible, and although up close this is a cool result, especially when you're walking in and around it, these transitions, yeah, they can be quite hard.

**[00:12:44]** So what you see here is a few wasteland tiles like meeting savanna tiles, and the artist and the art team had a really hard time just trying to alleviate these areas to get them as good as possible by creating additional tiles to sort of fit in between, And it became quite complex to set up and maintain, especially with more biomes and more variety coming in. And the drawback from the color map up close is also that it didn't quite have the resolution that we wanted to.

**[00:13:20]** So more complex planets meant more individual files to maintain. All of these collected assets informed the final look. So if you wanted to change a color on the planet, you had to go through all these individual files. Let's use the laser. So we had all these terrain files, these terrain sets. They had color information as well. So if we wanted the red on the planet to be a bit less red, we had to tweak all of those individual files. Could have meant like over 20 or even more.

**[00:13:50]** The overall look and final look at the planet was easily made up of 500 files that we all needed to maintain, keep track of, and made sure that they were all in sync. So you can imagine that this makes addressing feedback changes. Whenever I'm talking with Ian about art direction stuff, it becomes quite tedious to get all that stuff polished up and very time consuming.

## 00:14:14 — Separating climate and biomes from terrain shape

**[00:14:14]** So we had to do a fundamental rethinking of how we want to approach planets. And the first thing that we wanted to do was separate the process or the idea of a biome, temperature, whether it's lush or dry, from the actual terrain shape. By this, we could keep our terrain library clean, and we don't have to create additional color maps for every time we want to have a different color or a different type of terrain showing up. So, that way, you can get large changes on a global scale. This is not Google Earth anymore. This

**[00:14:53]** this is V4. And what we see here are natural transitions that occur globally due to gradual change in temperature, humidity, and weather conditions. And because these are occurring over a large scale, informed locally by the shape of the terrain, every area where it transitions or where a biome appears is informed by the shape of the terrain and is truly unique to that area. And we're no longer seeing a patch of dense savannah trees and next to

**[00:15:27]** it is the wasteland and you sort of see that stuff repeat. It's a large-scale transition and everything flows into each other. So we're no longer fixing a biome to a specific tile. And that way across the surface of the entire globe, every area truly becomes unique. This is an example of how extreme it could go with coloring, just as an example of some of the stuff you might see in the future. Just to reiterate this, another thing that we wanted

**[00:15:59]** to change, actually, is the blending, cross fading, transitioning between areas. And on the one hand, on the left side, we see V3 zooming out. On the other side, we see V4. Bear in mind, V4, this one is not entirely done. We would like to paint a bit more, but we had to come up with an example. And what you see here is a seamless zoom-out with no visual artifacts or things changing or fading or popping,

**[00:16:30]** whereas in V3, we still had a lot of transitions between all these layers of textures going on. And you can even see that at some point, the V3 planet, if you zoom out far enough, this global texture kicks in. It's the only one that remains. and it might actually have a completely different color from where you actually started. And I'm pretty sure people have experienced this as well. You see a spot on a planet, you wanna go there because it looks cool. You go there and it's actually a very different color experience than you expected. Whereas on V4, you can actually still make out the pattern,

**[00:17:05]** the exact terrain shape that we started with. And I think this is like a big, big difference. Okay. Let's move on to it. So those were the things that we really wanted to improve, and yeah, this one is another one that is especially important for the team. Like I said, managing and keeping track of 500 different files, doing color tweaks and all that stuff has become quite tedious, especially

**[00:17:39]** with complex planets. So, a very clean file structure with library items feeding into one file that is edited, authored, and maintained inside the editor in a very artist-friendly tool. So, proof in the pudding. Chris already spoiled it. V4 meant that all the planets that we've done in the last two and a half years since we started shipping planets had to be updated to v4, which was a bit of a -- you can get a bit nervous from it, right?

**[00:18:17]** That's a lot of work that we put into that. And it was the true test of how efficient this new planet tech would be and how efficient this tool would be. So I'm happy to confirm that the team has updated every single planet for 3.8 to V4, and it will be there.

## 00:18:44 — Terrain simulation, humidity, and temperature maps

**[00:18:44]** So just to get a bit of a start into the more technical areas, I just wanted to share a little bit what we do on the art creation side that feeds into what V4 does now. Already for the earlier version, V3, we used these little simulation tools that one of our in-house artists, Sebastian Schroeder, who had a big part in the Planet Tech V4, he couldn't make it but essential input, he made all these cool simulation tools that help us with terrain

**[00:19:15]** simulation. We see erosion, which is a very common simulation that you would do with building terrain. Water flow simulation. We see displacement of sand and soil based on wind input. And we see terracing. So these are just a few examples of the small little tools that we use in-house. And see? It helps. Up until now, these tools, they -- we use them just as like a personal thing, like you would have a brush inside your toolbox and you just use

**[00:19:49]** and you have an end result, an output that comes from it and we feed that into the engine. So we were able to get, like, if you're at the right distance, you get these beautiful terrain colors and terrain maps. So we're not unhappy with that. But every time we wanted to change something or wanted to make a different combination of colors or vegetation, you would have to redo it or make a new one. So instead of that making final outputs and specific masks, we decided to get those tools and simulate stuff and use the simulated data and feed

**[00:20:22]** that directly into the engine. So this was the old one. We had masks, we had color maps, specific patterns where certain assets would show up. And we started to rethink this and like, okay, how can we simplify this and make this easier for everything else to use? So we decided to go for humidity and temperature. There's hundreds of variations of things that you can think of that will cause transitions of areas and biomes, but I think temperature, whether something is hot or cold, whether

**[00:20:56]** it's dry or very humid, I think those are the two most important things. And if you look at some of the references that we looked at, even up close, the transitions are always happening because it gets colder, because there is water in the area, and even in desert areas where there is no water, those flow lines, they still are visible, because if there is water, it will trickle down into those pathways, stuff will sort of follow and get left behind in those areas. So I think with those two maps, we can capture a lot

**[00:21:28]** of interesting details. So this is the new set. Height, normals, just due to shading necessities, flow maps and temperature. So, you can see in this one -- let me just point there -- where the flow maps, where the water would flow, where water would accumulate in the valleys, and on the height. It's a relatively simple one. I think the temperature is more important on a global scale. But you see the differences in temperature if you go all the way up the hill or mountain. To make sure that every terrain set, every height map represents

**[00:22:07]** the exact same values with those flow maps and temperature maps, we had to unify all of our library height maps, basically. Our whole library of height maps. And we made

## 00:22:19 — Unified height-map dimensions and simulation values

**[00:22:19]** them all exactly four by four kilometers with a maximum height of one kilometer up and then maximum of one kilometer down. And the reason for doing this is that if we run these simulations on each height map, the data that comes out of that represents the exact same value for each height map. And this way, in the engine, we can actually blend these perfectly together and create this seamless canvas of data that we can paint on. And with that, I'm going

**[00:22:50]** hand it over to my colleague Ali, who will go into how that actually is handled inside the engine. Thanks a lot, Michel.

## 00:23:06 — Texture memory and planet-scale approaches — Alistair (Ali) Brown

**[00:23:06]** So yeah, to talk about how we make use of this climate data, first I want to take a step back and talk about the different approaches to how you can build a planet. So I've got got a line here showing all the various scales we need to accomplish. And a typical first person shooter, any old average small game, would typically be made of maybe two layers of textures. So you would have your small ground textures that might be representing millimeter accuracy details. And that would take you up to a meter or a couple of meters.

**[00:23:38]** And then they would have a next layer of texture, which would be their terrain. And you typically offer that, something like a one meter resolution. And then that would cover your entire play space, so say a kilometer for a small game. And that would typically account for about five megabytes of memory, depending on the tech you're using, which is obviously nice and cheap in this generation. But moving on to something a bit larger, a large open world game would typically stretch for 10 kilometers by 10 kilometers, or that type of scale, so 100 kilometers squared. And for something like this, you're looking for about 100 megabytes

**[00:24:10]** if you use the same approach. So that's still quite a sensible amount of memory, something you can easily achieve on a current-gen console and definitely on a PC. That's fine. That's not what we're doing, though. So what if we take the same approach and push it up to star-season scale? About 500 terabytes for a planet. So unless everyone's got some nice, meaty SSDs, you might be struggling there. So clearly that's not the way that anyone builds a planet. So what about -- Marco talked about procedural planets, like V1 was almost a fully procedural

**[00:24:41]** approach. It had limited art input. So if you build a fully procedural planet generation, what you tend to do is feed it with some very high-level data. It could be as simple as just the distance from the sun and the size, the composition of the crust. Or it could be more detailed, like a map of the continents, things like this. And then what you typically have is lots of layers of complex noise and algorithms that would procedurally determine all the details you'd actually get on the planet. So it would all be driven by algorithms, basically.

**[00:25:13]** No artist has ever hand-painted a mountain. It would just come from the system, which sounds amazing. Here's a very limited, quick example to show what type of small terrain sim and how you start with something lumpy and then it turns into something a bit nicer. I'll play that again. So that's a very small-scale example. So this sounds good. So with that type of approach, you can generate infinite planets, which is good. We get the whole solar system or whole universe filled.

**[00:25:44]** But actually, it's not as good as it seems. The variety you get is actually just limited by the complexity of the algorithm. So I could write an algorithm to say where rivers should spawn and how they should flow. And you might get a million different rivers, but you will never see a waterfall. And not until I go and then adjust the algorithm, add a waterfall. So you end up adding more and more layers of complexity, which is fine. But at some point, especially with a game like ours you're trying to simulate hundreds of different planets that are very unique. You know, we don't want 100 Earths or 100, you know, lunar moons.

**[00:26:15]** We want lava planets. We want, you know, all these radically different planets. So this approach doesn't really work for us. And what we actually want is something that's very art directed. You know, our art director, Ian, will tell us, you know, show us exactly what he wants, like a cliff to look like. It might be the slope of it, the shape of it, the colors. And we have to match that as the as the tech team. So doing something fully procedural doesn't really allow us to do that and it becomes a very technical, non-artistic process, which is quite limiting. So what

**[00:26:49]** does Star Citizen do? Well, up till V3 we combined four layers of textures, so typically we'd use the first few layers you would get in a normal first-person game, but then we had these two extra layers of textures that would stretch us all the way up to the global type of scale, and we blended all these together ever in the shader, and it cost us about 300 megs of memory, which is something that most GPUs can easily handle, so that is fine. But like Michel alluded to, it can be difficult to manage all these different layers

**[00:27:20]** and to blend them to try and achieve the art direction at the ground level, but then still to have a seamless transition to space. There was always these hard decisions we had to make, where if we wanted too much color variation on the surface, then from space we maybe could not achieve it. There was this constant balance now, so we need to do something here. So the fundamental approach for V4 is to make use of climate data.

## 00:27:42 — Climate-based rules and biome painting

**[00:27:46]** So we keep everything we had, but rather than, like Michel said, painting actual color maps, we now paint this climate data or simulate the climate data. And on top of that, we apply a global rule set for each planet that says, based on that climate, what would this planet do? So this can tell you everything from what the color of the ground would be, whether it's sand or snow, whether you might get a tree placed there or a rock. It informs us everything about the entire surface. And by using this top-down and bottom-up approach at the same time, we overcome all of the limitations that we had previously.

**[00:28:21]** Here's a 2D chart of temperature versus humidity. Michel mentioned we used these two properties because they seem the most relevant for most variations on a planet's surface, but we're quite loose with the term, the artists have a bit of flexibility what they put into them. And obviously some planets or moons especially, maybe humidity isn't relevant for them. We could theoretically just use any other single measure we want for climate there. But the point is we've got two axes, two things we can control. So what we do is then for each

**[00:28:52]** individual point on this 2D graph, we get to pick exactly what we want to appear. So Down here we might have, obviously, snow textures, whether we have different types of trees appearing in the temperate rainforest. We get to control the exact appearance at every single location within this chart. And we have 128 graduations of temperature and humidity, which leaves us with 16,000 variations we have to fill to tell you what would appear on the planet at that specific condition, which is quite a lot to fill.

**[00:29:24]** So here's a quick demo showing how a small terrain patch, just By adjusting the climate data, we can very quickly adjust the visuals. I'll just play that one again. And you see that we always get logical shapes and colors coming in there. So the problem with this now, it generates us a new problem. We've got 16,000 sets of conditions we need to set up somehow. So for the artists, that represented quite a challenge.

**[00:29:54]** Our first approach was for them to have-- they would manually create these rules where I want this tree to appear at this temperature range and at this altitude. And it was too unwieldy. So instead, we moved to a system where we literally paint the surface of the planet using a paintbrush. So this is a quick demo of this being used here. And because we don't want to paint the entire surface of the planet, when you paint on the surface, you're actually painting what you would like to appear at them climate conditions. So if you paint somewhere that's 20 degrees C

**[00:30:25]** and the humidity's 50%, you're actually painting every single point on the planet that has exactly those climate conditions. So by doing it this way, we can very quickly build up very interesting biomes, and while it looks like you're just painting the small layer of land in front of you, you're painting huge areas of the planet at once. And this was something that was immediately appealing to the artists, 'cause this is a very familiar workflow for anyone that's worked on a game engine before, like a smaller scale one.

**[00:30:56]** But yeah, it scales up for us. So here we've just painted a few trees and bushes, and instantly you can see the whole area has now been. - I think everybody did a little happy dance when that moment came in. The paint tool came in and was like, woohoo!

## 00:31:12 — Lookup tables, tree coverage, and normalized ground textures

**[00:31:12]** - So when they paint all this data, it goes into what we call a lookup table, which is just that 2D chart, and we generate a whole bunch of different ones. So here's a couple of them, or three of them. So we have the ground color, the type of surface it might be, so snow, rock, so that informs the physics engine of what to do there and what textures we should place. But then we also have things like tree coverage, so how much trees would spawn there so that when we, at space level, we can still draw the forests and draw the vegetation where typically most games would have to cull that stuff out, they just couldn't afford to deal with it.

**[00:31:43]** We can understand what the, you know, say if we're looking at the Amazon rainforest, we can understand what the color should be, 'cause we don't really want the ground color from space, like it might be brown in the rainforest, but we want the green of the lush trees above it. We can generate all this data, and we can use it at any altitude, and it gives us a lot of power. Also, this rich information we can use for various other gameplay effects, which you will see a bit more of later today. We saw a sneak peek on our first demo of how temperature can drive things, but there is a lot to come with these planet climate conditions.

**[00:32:15]** Here is a visualization of the climate on Cellin. The red and green are just visualizations of the temperature and humidity. And one of the lookup tables is shown there, which shows that thing as the ground color. So once we apply that to the surface, it starts to look a lot more reasonable. And then same for microTech, we've got all the climate conditions there. It's mostly much snow in microTech, no surprise, but yeah. And then to build it up from the surface to see the other side of it.

**[00:32:45]** So this is purely just the terrain plus the global lookup table. So it looks pretty boring at the moment. Once we apply some temperature variations, there's a little bit more interest. The humidity variations gives us quite a lot. And then on top of that, we have a medium scale type of terrain textures, which are driven from the climate and the slope. And then finally, we have the detail textures. Now, our climate data is only stored at 4 meter resolution. So that doesn't give us the individual stones and rocks.

**[00:33:17]** We have to add a layer of detailed ground texture. But we need them textures to look consistent with what the climate data tells us. So if the climate table tells us we should be having yellow sand here, then we may need to make sure we have yellow sand. Now, we can't make a texture for every single scenario. So the solution for that is we normalize all of our ground textures so that they all have an average of mid-gray color, which you can see in the top left. And then we have an average amount of bumpiness, which is the middle one, and the right-hand side

**[00:33:48]** shows the roughness. So they're all type of normalized, so they have this equal amount of stuff, whatever it is they're storing in that Texture. And then when we come to apply it to the surface, we type of use a physically-based algorithm to rescale them to achieve what the lookup table told us it wanted or what the artist wanted, but we preserve all the details that were in the original Texture. So in the color map, that might be slight hue variations, and in the roughness map, there might be like pebbles and stones and sand of different densities roughness so this allows us to keep all of that detail but make sure it's

**[00:34:19]** consistent and that consistency is really important for us we actually apply the same concept as Michel mentioned at the four kilometer scale so here's a very basic example of a bunch of different terrain tiles tiled up next to each other and obviously there's a very hard join between them but because we've made the climate data normalized and in the same range when we try to combine them together we get a nice seamless transition and then we apply the climate data ruleset on top of that, then it feels completely natural, and even in this

**[00:34:51]** very primitive example, it's very hard to see the transitions, and we get a very natural and logical progression from the terrain, which is really nice.

## 00:35:00 — Variance maps and consistent surface-to-space shading

**[00:35:00]** There is one slight flaw of the technique, which is, say, in this little highlighted box I've got on the bottom right, if I want to have a mountain range that's going to cover multiple biomes, multiple climate conditions, if I zoom out to space, I don't want to just get the average color of that region, I want to make sure to see all the details that would have been in there. So in this, I've got a combination of shrubland and snow and polar desert. So depending on how varied that terrain was, I should be seeing a mixture of them colors. But just using the average color, which is what you would tend to get in the engine without

**[00:35:31]** any extra tech, you would just get the color of that polar region in the middle, polar desert. So to solve this, we came up with a statistical model, which we call variance maps, which is shown on the right-hand side here, which is where we store the variation from inside a texture. So the left-hand side, it shows us the humidity of a particular patch of terrain, and the right-hand side is showing us the brighter areas is where the climate is varying highly in that area. And then as we increase in altitude, the hardware, the GPU hardware, naturally uses lower resolution

**[00:36:06]** textures to cover the smaller scale. So then the variation map then increases in brightness to cover the amount of variation that is within one pixel now. So as we go, now it's a 32 by 32 texture. You can see that it's much brighter now, and it's still-- the areas that are white now are the areas that had a lot of variation previously. And when it comes down to actually applying the climate rule set, rather than just applying the rule for the particular location of the particular climate data we have now,

**[00:36:37]** we actually use these variation maps sum up an area of the climate data, which we use the GPU's anisotropic filtering for that. And that allows us to sample all of the details that would have been within one pixel and make sure it's faithful to the original image, so that when you get down to the surface and you might have 50% snow, 50% grass, when you zoom out you'll get the exact blend you would expect or an approximation of the blend of snow and grass. So yeah, that really helps us make sure that we have that consistent view from space and

**[00:37:10]** a nice softness and it avoids the need to have a blend to some other texture when you're far away. So that's the real key thing we get from v4 in my eyes. So I'm going to hand back to Marco now to talk about how we put all this together and how we turn this into a planet. Cool.

## 00:37:25 — Planet generation, ecosystem blending, and object scattering — Marco Corbetta

**[00:37:25]** Thank you, Ali. Okay. Cool. Let's have a quick look at the planet generation. So we are starting from a cube. Each cube face is projected onto a sphere. The entire planet's surface is generated and rendered at different levels of details. And the amount of details increases as you get closer to the surface. Here you can see a debug screenshot showing the surface level

**[00:37:56]** humidity, as we discussed before. In V4, we are blending all nearby ecosystems for all terrain vertices using a bicubic interpolation approximation. Here is the same view, but we're showing temperature climate data. Here is another improvement, as we mentioned, that we made in V4, is that object scattering is driven by the same climate data that we are using for terrain generation. Here you can see the object preset IDs, driving objects

**[00:38:31]** spawning on demand. This is the same view, showing the colours driven by climate data and textures. And here you can see the wireframe mesh geometry generated from elevation data. The terrain geometry and blending is done on CPU. This is the same view as before, showing the planet terrain without any objects yet. And this is the final view with objects generated on terrain. So we have a separation between large-scale ecosystems, which are the larger

**[00:39:07]** rocks, trees, and so on. And then we have the so-called ground cover objects, which are the smaller objects that are generated at ground level only. Additionally, we have improved the cliff side generation. The objects are placed based on climate data and object preset settings, and we are using LOD clusters for large-scale forest rendering which you will see in microTech. At ground level, we have additional parallax and ground texture

## 00:39:40 — On-demand physics collision geometry

**[00:39:40]** details. So, physics. The collision geometry is generated on demand on client and servers when players and spaceships are interacting with the physics grid. And here in light grey, you can see the physics proxies that are generated for terrain, rocks and objects. So generating physics on demand is tricky because the spaceships are actually moving at crazy speeds across the surface,

**[00:40:11]** and obviously we cannot store the entire planet geometry in memories, otherwise would be terabytes of data. So each terrain patch is built in parallel on CPU. The workload is distributed by the job system to all available cores, and basically the server is building a physical collision mesh for each client. So we have many other features

## 00:40:37 — Caves, exclusion volumes, and object containers

**[00:40:37]** on the planets, for instance, caves. Here you can see an example of a cave assembled by the procedural layout tool. After generation, the caves are turned into object containers and they're placed on the planet's surface. So using object containers, we can take advantage of object container streaming in future. Here you can see an example of a cave with an exclusion volume which is used to avoid generating rocks on top of the cave entrance. Here is another

**[00:41:11]** For example of exclusion volumes, you can see how the trees are avoiding generating inside the volumes. Additionally, we have space stations orbiting around planets. All the space station interiors are generated by the procedural layout tool as well and then are placed as object containers in space.

## 00:41:37 — Frozen oceans and mineable entities

**[00:41:37]** Another new feature we added in V4 is the frozen ocean, which can be seen here on microTech. The frozen ocean is physicalised and players can walk, drive vehicles on it. In these pictures, we're showing the frozen ocean collision mesh generated on demand. So regarding mining, the way it works is that some of the procedural objects are turned into mineable entities, and the player can interact with them when extracting minerals.

**[00:42:10]** And with persistency coming online, players are able to deplete planet resources affecting the universe economy. So let's show some of the rework, which is coming with 3.8.

## 00:42:25 — Alpha 3.8 planet rework and closing remarks

**[00:42:25]** I'm just going to let this run a little bit. Obviously we wanted to at least close this talk with showing you guys the current state of all the rework. Like I said, the team has been working hard and is still working at it. We want to make sure that we deliver the best possible Planet experience for 3.8 coming up. But we're already seeing a lot of the the features that came in with V4, making everything, every location that we've known

**[00:42:57]** or built up until now looking way better. And there's a lot of subtle differences that might not be apparent at first glance, but we just wanted to reassure you guys, all the planets are updated and they will be coming. So I guess we could already start thanking everybody who's been involved with this because it's been, to summarize, it's been like a full year working on this with some very high-level people and a lot of people involved with this.

**[00:43:28]** So this was not an easy undertaking. Yeah, it was a team effort. So thanks for all the people on the organics team. Thanks to Marco, Ali, Sebastian Schroeder, all the people on the graphics and engineering and we hope you have a great time with the new planets yeah I hope you guys have enjoyed this presentation thank you for listening and enjoy the rest of the show thank you thank you

