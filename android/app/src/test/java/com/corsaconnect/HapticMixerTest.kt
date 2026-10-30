package com.corsaconnect

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNotNull
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

/** Plays situations through [HapticMixer] at 60 Hz on a fake clock. */
class HapticMixerTest {

    private val cfg = HapticSettings()

    private fun state(
        beam: Protocol.BeamHaptics? = null, gear: Int = 3, rpm: Float = 3000f,
        slip: Float = 0f, impact: Float = 0f, shift: Boolean = false, clutch: Float = 0f,
    ) = HapticState(true, rpm, gear, clutch, 0f, shift, slip, impact, beam)

    private fun beam(rumble: Float = 0f, spin: Float = 0f, lock: Float = 0f, abs: Boolean = false) =
        Protocol.BeamHaptics(rumble, spin, lock, abs)

    /** Runs [frames] ticks 16 ms apart; returns the commands with their time. */
    private fun run(
        mixer: HapticMixer, frames: Int, start: Long = 0, settings: HapticSettings = cfg,
        at: (Int) -> HapticState,
    ): List<Pair<Long, HapticCommand>> =
        (0 until frames).mapNotNull { i ->
            val t = start + i * 16L
            mixer.update(at(i), settings, t)?.let { t to it }
        }

    private fun plays(cmds: List<Pair<Long, HapticCommand>>) = cmds.count { it.second is HapticCommand.Play }

    @Test fun steadyRoadStartsOnceAndKeepsLooping() {
        val m = HapticMixer(amplitudeControl = false)
        val cmds = run(m, 120) { state(beam(rumble = 0.5f)) } // 2 s of the same road
        assertEquals("one loop, not a vibrate() per frame", 1, cmds.size)
        val p = (cmds[0].second as HapticCommand.Play).pattern
        assertEquals("a repeating loop", 0, p.repeat)
        assertTrue("pulses far enough apart to feel", p.durationMs >= 50)
    }

    @Test fun aKerbIsFeltLongEnoughThenStops() {
        val m = HapticMixer(amplitudeControl = false)
        // BeamNG's jerk: one frame at full, then nothing.
        val cmds = run(m, 60) { i -> state(beam(rumble = if (i == 5) 1f else 0f)) }
        val start = cmds.first { it.second is HapticCommand.Play }.first
        val stop = cmds.first { it.second == HapticCommand.Stop }.first
        assertTrue("held at least 100 ms, was ${stop - start}", stop - start >= 100)
        assertTrue("and gone within 300 ms, was ${stop - start}", stop - start <= 300)
    }

    @Test fun absHasItsOwnChatter() {
        val m = HapticMixer(amplitudeControl = false)
        val p = (run(m, 10) { state(beam(abs = true, rumble = 0.3f)) }.first().second as HapticCommand.Play).pattern
        assertEquals(m.texture(HapticMixer.Texture.ABS, HapticMixer.STEPS), p)
        assertEquals("~15 Hz", 66L, p.durationMs)
    }

    @Test fun lockUpOverridesALightRoad() {
        val m = HapticMixer(amplitudeControl = false)
        val p = (run(m, 5) { state(beam(rumble = 0.1f, lock = 0.12f)) }.first().second as HapticCommand.Play).pattern
        val lockLoop = (1..HapticMixer.STEPS).map { m.texture(HapticMixer.Texture.LOCKUP, it) }
        assertTrue("lock-up texture plays", p in lockLoop)
    }

    @Test fun wheelspinAloneIsAFineGrain() {
        val m = HapticMixer(amplitudeControl = false)
        val p = (run(m, 5) { state(beam(spin = 0.2f)) }.first().second as HapticCommand.Play).pattern
        assertTrue(p in (1..HapticMixer.STEPS).map { m.texture(HapticMixer.Texture.WHEELSPIN, it) })
        assertTrue("short pulses", p.timings[1] <= 16)
    }

    @Test fun aGearChangeCutsInAndTheRoadComesBack() {
        val m = HapticMixer(amplitudeControl = false)
        val cmds = run(m, 60) { i -> state(beam(rumble = 0.5f), gear = if (i < 20) 3 else 4) }
        assertEquals(3, plays(cmds)) // road, the knock, road again
        val knock = cmds[1]
        assertEquals("not a loop", -1, (knock.second as HapticCommand.Play).pattern.repeat)
        val back = cmds[2].first - knock.first
        assertTrue("road resumes after the knock, ${back} ms", back >= (knock.second as HapticCommand.Play).pattern.durationMs)
    }

    @Test fun theSlideEstimateOnlyStandsInWithoutBeamNG() {
        val withBeam = run(HapticMixer(false), 20) { state(beam(), slip = 0.3f) }
        assertTrue("BeamNG says calm: nothing", withBeam.isEmpty())
        val without = run(HapticMixer(false), 20) { state(null, slip = 0.3f) }
        assertEquals(1, plays(without))
    }

    @Test fun aLevelOnAStepEdgeDoesNotRestartTheLoop() {
        val m = HapticMixer(amplitudeControl = false)
        // Jitters across the 0.4 boundary between steps 2 and 3 every frame.
        val cmds = run(m, 120) { i -> state(beam(rumble = if (i % 2 == 0) 0.39f else 0.41f)) }
        assertTrue("restarted ${plays(cmds)} times", plays(cmds) <= 2)
    }

    @Test fun intensityShortensPulsesWithoutAmplitudeControl() {
        val full = (run(HapticMixer(false), 3) { state(beam(rumble = 0.6f)) }.first().second as HapticCommand.Play).pattern
        val soft = (run(HapticMixer(false), 3, settings = cfg.copy(intensity = 0.2f)) { state(beam(rumble = 0.6f)) }
            .first().second as HapticCommand.Play).pattern
        assertTrue("on ${soft.timings[1]} < ${full.timings[1]}", soft.timings[1] < full.timings[1])
        assertEquals("same beat", full.durationMs.toDouble(), soft.durationMs.toDouble(), 1.0)
    }

    @Test fun switchedOffStopsOnceThenStaysQuiet() {
        val m = HapticMixer(false)
        run(m, 5) { state(beam(rumble = 0.6f)) }
        val off = run(m, 30, start = 100, settings = cfg.copy(enabled = false)) { state(beam(rumble = 0.6f)) }
        assertEquals(listOf<HapticCommand>(HapticCommand.Stop), off.map { it.second })
    }

    @Test fun aCrashSlams() {
        val m = HapticMixer(false)
        val cmds = run(m, 10) { i -> state(impact = if (i >= 3) 0.8f else 0f) }
        val p = (cmds.single().second as HapticCommand.Play).pattern
        assertTrue("long hard hit", p.timings[1] >= 200)
        assertNotNull(p)
    }

    @Test fun nothingHappeningIsSilent() {
        assertNull(run(HapticMixer(false), 60) { state(beam()) }.firstOrNull())
    }
}
