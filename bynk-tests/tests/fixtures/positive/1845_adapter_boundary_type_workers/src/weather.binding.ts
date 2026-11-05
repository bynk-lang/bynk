import type { Weather, Report } from "./weather.js";
import { WeatherError } from "./weather.js";
import { Ok, Err, type Result } from "./runtime.js";

export class FixedWeather implements Weather {
  async current(city: string): Promise<Result<Report, WeatherError>> {
    if (city === "") {
      return Err(WeatherError.NotFound);
    }
    return Ok({ tempC: 21, summary: "clear" });
  }
}
