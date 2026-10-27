// C shim over Bungee's granular API for opendeck-timestretch.
//
// Bungee's native API is pull/granular: the caller asks for one grain at a
// time, naming the input position, speed and pitch, and gets a chunk of
// output back.  That is what a vinyl-mode engine wants (reverse and zero
// speed are free), but the deck's pipeline is push-based today: it hands the
// stretcher 512 input frames and takes whatever comes out.  This shim is the
// push adapter: it keeps a short history of pushed input, and for every push
// renders `inFrames / speed` output frames by running as many grains as
// that takes.  Each grain is specify → analyse → synthesise in one go, so
// the stretcher is never left mid-grain between calls and a reset is just
// "forget the history, restart on the next grain".
//
// (Bungee ships `Bungee::Stream` for the same purpose, but it leaves a grain
// half-done between calls, which makes a reset abort inside the library.)
#include <bungee/Bungee.h>

#include <algorithm>
#include <cmath>
#include <limits>
#include <vector>

namespace {

struct Shim
{
	Bungee::Stretcher<Bungee::Basic> stretcher;
	const int channels;
	const int maxInput;   // largest input frame count per push
	const int window;     // stretcher.maxInputFrameCount(): the input span a grain can ask for
	const int stride;     // history capacity per channel

	// Input history, per channel at `hist[c * stride]`, covering absolute
	// input positions [histBegin, histEnd).
	std::vector<float> hist;
	long long histBegin = 0, histEnd = 0;

	Bungee::Request request{};
	bool restart = true;        // next grain must carry Request::reset (with a one-hop pre-roll)
	double outPos = 0.;         // input position the delivered output has reached
	double origin = 0.;         // output before this input position is discarded (pre-roll after a reset)
	double framesOwed = 0.;     // fractional output frames not yet rendered

	// Last synthesised grain, copied out of the stretcher, per channel at
	// `chunk[c * chunkStride]`, `chunkUsed` frames already delivered.
	std::vector<float> chunk;
	int chunkStride = 0, chunkFrames = 0, chunkUsed = 0;
	double chunkPos0 = std::numeric_limits<double>::quiet_NaN(), chunkPos1 = chunkPos0;

	Shim(int sampleRate, int channels, int maxInput) :
		stretcher({sampleRate, sampleRate}, channels, 0),
		channels(channels),
		maxInput(maxInput),
		window(stretcher.maxInputFrameCount()),
		stride(stretcher.maxInputFrameCount() + maxInput),
		hist(size_t(stride) * channels, 0.f)
	{
		reset();
	}

	// Forget everything: the next pushes count from input position 0 again,
	// nothing comes out until half a window of input has arrived, and the
	// first grain is pre-rolled one hop before 0 so the start is not weak.
	void reset()
	{
		histBegin = histEnd = 0;
		request.position = std::numeric_limits<double>::quiet_NaN();
		restart = true;
		outPos = origin = 0.;
		framesOwed = 0.;
		chunkFrames = chunkUsed = 0;
		chunkPos0 = chunkPos1 = std::numeric_limits<double>::quiet_NaN();
	}

	void append(const float *const *in, int n)
	{
		if (n <= 0)
			return;
		const long long keep = std::min<long long>(histEnd - histBegin, stride - n);
		if (histEnd - histBegin > keep)
		{
			const long long drop = (histEnd - histBegin) - keep;
			for (int c = 0; c < channels; ++c)
				std::move(&hist[c * stride + drop], &hist[c * stride + (histEnd - histBegin)], &hist[c * stride]);
			histBegin += drop;
		}
		const long long at = histEnd - histBegin;
		for (int c = 0; c < channels; ++c)
			if (in)
				std::copy(in[c], in[c] + n, &hist[c * stride + at]);
			else
				std::fill(&hist[c * stride + at], &hist[c * stride + at + n], 0.f);
		histEnd += n;
	}

	// Run one grain centred on `position` and copy its output into `chunk`.
	void grain(double position, double speed, double pitch)
	{
		request.reset = restart || !(position > request.position);
		request.position = position;
		request.speed = speed;
		request.pitch = pitch;
		request.resampleMode = resampleMode_autoOut;
		restart = false;

		const Bungee::InputChunk ic = stretcher.specifyGrain(request, 0.);
		// Frames the stretcher wants that we do not have are muted.
		const int muteHead = int(std::max<long long>(0, histBegin - ic.begin));
		const int muteTail = int(std::max<long long>(0, ic.end - histEnd));
		// Pointer for absolute position ic.begin; the stretcher only reads
		// [muteHead, length - muteTail) of it (same convention as Bungee::Stream).
		const float *data = hist.data() + (ic.begin - histBegin);
		stretcher.analyseGrain(data, stride, muteHead, muteTail);

		Bungee::OutputChunk oc{};
		stretcher.synthesiseGrain(oc);

		chunkFrames = oc.frameCount;
		chunkUsed = 0;
		chunkStride = oc.frameCount;
		if (chunk.size() < size_t(chunkStride) * channels)
			chunk.resize(size_t(chunkStride) * channels);
		for (int c = 0; c < channels; ++c)
			std::copy(oc.data + c * oc.channelStride, oc.data + c * oc.channelStride + oc.frameCount, &chunk[c * chunkStride]);
		chunkPos0 = oc.request[0] ? oc.request[0]->position : std::numeric_limits<double>::quiet_NaN();
		chunkPos1 = oc.request[1] ? oc.request[1]->position : chunkPos0;
	}

	// Push `inFrames` frames and render what they unlock: the output reaches
	// half a window behind the newest input, so in steady state that is
	// `inFrames / speed` frames, and nothing until the first half window after
	// a reset (like Rubber Band's latency, which the deck pre-rolls through).
	int process(const float *const *in, float *const *out, int inFrames, double speed, double pitch, int capacity)
	{
		append(in, inFrames);
		speed = std::max(speed, 1e-3);
		const double targetPos = double(histEnd) - window / 2.;
		if (targetPos <= outPos)
			return 0;
		framesOwed += (targetPos - outPos) / speed;
		const int target = std::min(int(std::lround(framesOwed)), std::max(capacity, 0));

		int done = 0;
		while (done < target)
		{
			if (chunkUsed >= chunkFrames || std::isnan(chunkPos0))
			{
				double position;
				if (restart)
				{
					Bungee::Request r{};
					r.position = outPos;
					r.speed = speed;
					r.pitch = pitch;
					stretcher.preroll(r); // one hop before the start, reset flagged
					position = r.position;
				}
				else
					position = targetPos - speed * (framesOwed - done);
				grain(position, speed, pitch);
				if (std::isnan(chunkPos0) || chunkFrames == 0)
					continue; // no output from this grain yet
				if (chunkPos0 < origin && chunkPos1 > chunkPos0)
				{
					// Pre-roll output: skip the frames before the start.
					const double f = (origin - chunkPos0) / (chunkPos1 - chunkPos0);
					chunkUsed = std::min(chunkFrames, int(std::ceil(f * chunkFrames)));
				}
				continue;
			}
			const int n = std::min(target - done, chunkFrames - chunkUsed);
			for (int c = 0; c < channels; ++c)
				std::copy(&chunk[c * chunkStride + chunkUsed], &chunk[c * chunkStride + chunkUsed + n], out[c] + done);
			done += n;
			chunkUsed += n;
		}
		framesOwed -= done;
		outPos = targetPos - framesOwed * speed;
		return done;
	}

	// Input frames between the newest input and the delivered output.  Half a
	// window structurally (that is also how much input a reset needs before
	// anything comes out), plus whatever fraction is still owed.
	double latency() const
	{
		return std::max(window / 2., double(histEnd) - outPos);
	}
};

} // namespace

extern "C" {

void *od_bungee_create(int sampleRate, int channels, int maxInputFrames)
{
	return new Shim(sampleRate, channels, maxInputFrames);
}

void od_bungee_destroy(void *p)
{
	delete static_cast<Shim *>(p);
}

const char *od_bungee_version(void)
{
	return Bungee::Stretcher<Bungee::Basic>::version();
}

// Push `inFrames` frames (per-channel pointers, or null for silence) at
// `speed` (1 = unchanged, >1 faster) and `pitch` (frequency multiplier),
// rendering up to `capacity` frames into `out[c]`.  In steady state that is
// about inFrames / speed; after a reset nothing comes out until half a window
// of input has been pushed.  Returns the number of frames rendered.
int od_bungee_process(void *p, const float *const *in, float *const *out, int inFrames, double speed, double pitch, int capacity)
{
	return static_cast<Shim *>(p)->process(in, out, inFrames, speed, pitch, capacity);
}

// Latency in input frames: how far the delivered output lags the input pushed so far.
double od_bungee_latency(void *p)
{
	return static_cast<Shim *>(p)->latency();
}

void od_bungee_reset(void *p)
{
	static_cast<Shim *>(p)->reset();
}

} // extern "C"
