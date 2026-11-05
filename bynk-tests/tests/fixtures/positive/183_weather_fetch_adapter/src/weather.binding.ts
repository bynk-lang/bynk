import type { Weather, Report } from "./weather.js";
import { WeatherError } from "./weather.js";
import type { Fetch, Secrets } from "./bynk.js";
import { Method } from "./bynk.js";
import { Ok, Err, None, Some, type Result } from "./runtime.js";

export class FetchWeather implements Weather {
  // v0.18 ([M]/[N]): outbound IO and the API key are capability
  // dependencies (bynk.Fetch / bynk.Secrets), not raw fetch + params. Declared
  // field + assigning constructor (not a parameter property), so the binding
  // strips cleanly under Node `--experimental-strip-types`.
  private deps: { Fetch: Fetch; Secrets: Secrets };
  constructor(deps: { Fetch: Fetch; Secrets: Secrets }) {
    this.deps = deps;
  }

  async current(city: string): Promise<Result<Report, WeatherError>> {
    const key = await this.deps.Secrets.get("WEATHER_API_KEY");
    const res = await this.deps.Fetch.send({
      method: Method.Get,
      url: `https://api.example.com/weather?city=${encodeURIComponent(city)}`,
      contentType: None,
      authorization: key.tag === "Some" ? Some(`Bearer ${key.value}`) : None,
      headers: new Map<string, string>(),
      body: None,
    });
    if (res.tag === "Err") {
      return Err(WeatherError.Upstream);
    }
    if (res.value.status === 404) {
      return Err(WeatherError.NotFound);
    }
    if (res.value.status !== 200) {
      return Err(WeatherError.Upstream);
    }
    const body = JSON.parse(res.value.body) as { temp_c: number; summary: string };
    return Ok({ tempC: Math.trunc(body.temp_c), summary: body.summary });
  }
}
